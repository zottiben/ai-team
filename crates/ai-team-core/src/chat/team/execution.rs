//! Approved work only: bounded parallel makers, one verifier seat, and explicit cleanup.
//! No legacy planner/dispatcher, remote delivery, or merge into the solo checkout.

pub(super) mod git;
mod worker;
pub(super) use worker::resume;

use crate::{ChatBuildControl, ChatBuildStart, Error, Result, Store};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinHandle};

#[derive(Clone)]
pub(super) struct Watch {
    db: PathBuf,
    pub(super) control: ChatBuildControl,
    pub(super) cleanup: bool,
}
impl Watch {
    pub(super) fn new(db: &Path, control: &ChatBuildControl) -> Self {
        Self {
            db: db.to_owned(),
            control: control.clone(),
            cleanup: false,
        }
    }
    fn check_store(&self, store: &Store) -> Result<()> {
        if self.cleanup {
            return store.check_chat_build_cleanup(&self.control);
        }
        if store.chat(self.control.receipt.chat_id)?.stop_requested {
            return Err(Error::invalid(
                "Stopped by you; unfinished work is kept for recovery.",
            ));
        }
        if self.control.recovering {
            store.check_chat_build_cleanup(&self.control)?;
        } else {
            store.check_chat_build(&self.control)?;
        }
        Ok(())
    }
    pub(super) fn check(&self) -> Result<()> {
        self.check_store(&Store::open(&self.db)?)
    }
    pub(super) async fn wait(self) -> String {
        let store = match Store::open(&self.db) {
            Ok(store) => store,
            Err(error) => return error.to_string(),
        };
        loop {
            if let Err(error) = self.check_store(&store) {
                return error.to_string();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Drive the exact human-approved execution once. Failures retain their work/ownership
/// for explicit recovery; a successful run returns only verified, committed leases.
pub async fn drive_chat_team_build(db: &Path, start: ChatBuildStart) -> Result<()> {
    let mut store = Store::open(db)?;
    let mut control = store.claim_chat_build(&start)?;
    let outcome = build(&mut store, &control).await;
    let reason = outcome.as_ref().err().map(ToString::to_string);
    store
        .finish_chat_build(&mut control, reason.as_deref())
        .map_err(|cleanup| {
            Error::invalid(format!(
                "{}; settling build ownership failed: {cleanup}",
                reason.as_deref().unwrap_or("Workers finished")
            ))
        })?;
    outcome
}

struct Wave(Vec<(String, JoinHandle<Result<()>>)>);
impl Drop for Wave {
    fn drop(&mut self) {
        for (_, task) in &self.0 {
            task.abort();
        }
    }
}

async fn build(store: &mut Store, control: &ChatBuildControl) -> Result<()> {
    let run = store.run(control.receipt.run_id)?;
    let order: HashMap<_, _> = store
        .chat_plan(control.receipt.chat_id, crate::planning::PlanActor::Human)?
        .bundle
        .ok_or_else(|| Error::invalid("the approved plan disappeared"))?
        .slices
        .into_iter()
        .map(|slice| (slice.id, slice.ord))
        .collect();
    let mut approved = store.chat_build_slices(run.id)?;
    approved.sort_by_key(|slice| {
        order
            .get(&slice.planner_slice_id)
            .copied()
            .unwrap_or(i64::MAX)
    });
    let mut queue = VecDeque::from(approved);
    let verifier = Arc::new(Semaphore::new(1));
    let mut errors = Vec::new();
    while !queue.is_empty() {
        let mut busy = HashSet::new();
        let mut wave = Wave(Vec::new());
        // Examine every queued item once. Busy-role items stay queued, not discarded.
        for _ in 0..queue.len() {
            let slice = queue
                .pop_front()
                .ok_or_else(|| Error::invalid("the build queue disappeared"))?;
            if wave.0.len() >= usize::try_from(run.parallel_width.max(1)).unwrap_or(1)
                || !busy.insert(slice.assigned_agent_id)
            {
                queue.push_back(slice);
                continue;
            }
            let db = store.path().to_owned();
            let receipt = control.clone();
            let seat = verifier.clone();
            let key = slice.slice_key;
            let task_key = key.clone();
            wave.0.push((
                key,
                tokio::spawn(async move {
                    receipt
                        .ownership
                        .track(worker::run(&db, &receipt, &task_key, seat))
                        .await
                }),
            ));
        }
        for (key, task) in &mut wave.0 {
            let failure = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(error) => {
                    control.ownership.doubt();
                    Some(format!("worker task failed: {error}"))
                }
            };
            if let Some(reason) = failure {
                // Reporting is fallible too. Never ? out of this loop and detach siblings.
                if let Err(error) = store.fail_chat_slice_worker(control, key, &reason) {
                    errors.push(format!("{key}: recording failure: {error}"));
                }
                if let Err(error) = store.settle_chat_build_board(control, key, Some(&reason)) {
                    errors.push(format!("{key}: recording board outcome: {error}"));
                }
                errors.push(format!("{key}: {reason}"));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::invalid(errors.join("\n")))
    }
}
