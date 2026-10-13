use super::{Error, HashMap, Mutex, Result, Ring, Session, Terminals};
use serde::Serialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Explicit permission to close these app-owned sessions, never coding-agent turns.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalUpdate {
    pub approval: String,
    pub sessions: Vec<UpdateSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateSession {
    pub id: u64,
    pub label: String,
    pub worktree: String,
}

impl Terminals {
    pub fn review_update(&self) -> TerminalUpdate {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut open: Vec<_> = sessions
            .iter()
            .filter_map(|(id, session)| {
                let output = session
                    .output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (!output.settled || !output.drained).then(|| UpdateSession {
                    id: *id,
                    label: session.label.clone(),
                    worktree: session.worktree.clone(),
                })
            })
            .collect();
        open.sort_by_key(|session| session.id);
        TerminalUpdate {
            approval: self.approval(&sessions),
            sessions: open,
        }
    }

    fn approval(&self, sessions: &HashMap<u64, Session>) -> String {
        let mut ids: Vec<_> = sessions.keys().copied().collect();
        ids.sort_unstable();
        // Natural completion may remove a blocker, but never expands this authority.
        crate::update::sha256(format!("{}:{ids:?}", self.scope).as_bytes())
    }

    pub(crate) fn close_for_update(&self, expected: &str) -> Result<()> {
        let outputs = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.approval(&sessions) != expected {
                return Err(Error::invalid(
                    "app terminal sessions changed; review the update again before closing them",
                ));
            }
            let mut outputs = Vec::new();
            for session in sessions.values_mut() {
                stop(session)?;
                outputs.push(Arc::clone(&session.output));
            }
            outputs
        };
        // Keep scrollback mounted. Permits are released only by wait + output drain,
        // not by this request, a timeout, or removal from the session list.
        drain(&outputs)
    }
}

#[cfg(unix)]
pub(super) fn group_drained(group: Option<u32>) -> bool {
    let group = group
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .and_then(rustix::process::Pid::from_raw);
    group.is_some_and(|group| {
        rustix::process::test_kill_process_group(group) == Err(rustix::io::Errno::SRCH)
    })
}

pub(super) fn stop(session: &mut Session) -> Result<()> {
    let mut child = session
        .child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    session.closing = true;
    if child.try_wait()?.is_none() {
        child.kill().map_err(|error| {
            Error::invalid(format!(
                "could not close {}: {error}; its update protection is retained",
                session.label
            ))
        })?;
    }
    Ok(())
}

pub(super) fn drain(outputs: &[Arc<Mutex<Ring>>]) -> Result<()> {
    let started = Instant::now();
    loop {
        let mut finished = true;
        for output in outputs {
            let output = output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(problem) = &output.problem {
                return Err(Error::invalid(format!("terminal drain failed: {problem}; update protection and scrollback are retained")));
            }
            finished &= output.settled && output.drained;
        }
        if finished {
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(5) {
            return Err(Error::invalid("an app terminal or sign-in session has not exited and drained; its update protection and scrollback are retained. No programs were replaced"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
