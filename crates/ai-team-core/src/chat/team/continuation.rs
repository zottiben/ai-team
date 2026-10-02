//! Explicit human continuation is neither process recovery nor another build approval.

use crate::{ChatBuildControl, ChatBuildRecovery, Error, Result, Store};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatBuildResume {
    pub target: ChatBuildRecovery,
    pub slice_key: String,
    pub expect_slice_revision: i64,
}

/// Continue only this retained slice. No new lease, unrelated ready work, or reset.
/// Failed validation leaves the run blocked and cannot grant a model permission.
pub async fn resume_chat_team_slice(db: &Path, request: &ChatBuildResume) -> Result<()> {
    ChatBuildContinuation::claim(db, request.clone())?
        .run()
        .await
}

/// A Rust-only continuation receipt. Human transports claim before acknowledging
/// admission, then hand this owned receipt to a watched task without an await gap.
/// Physical lease/policy checks still precede model authority inside `run`.
#[derive(Debug)]
pub struct ChatBuildContinuation {
    store: Store,
    control: ChatBuildControl,
    request: ChatBuildResume,
}

impl ChatBuildContinuation {
    pub fn claim(db: &Path, request: ChatBuildResume) -> Result<Self> {
        let mut store = Store::open(db)?;
        let control = store.claim_quiescent_chat_build(&request.target, Some(&request), false)?;
        Ok(Self {
            store,
            control,
            request,
        })
    }

    pub async fn run(self) -> Result<()> {
        let Self {
            mut store,
            mut control,
            request,
        } = self;
        let db = store.path().to_owned();
        continue_claimed(&db, &mut store, &mut control, &request).await
    }
}

async fn continue_claimed(
    db: &Path,
    store: &mut Store,
    control: &mut ChatBuildControl,
    request: &ChatBuildResume,
) -> Result<()> {
    let outcome = control
        .ownership
        .clone()
        .track(super::execution::resume(db, control, request))
        .await;
    let reason = outcome.as_ref().err().map(ToString::to_string);
    let mut problems: Vec<_> = reason.iter().cloned().collect();
    // Refused validation must not mutate the slice or release its claim. Once a
    // worker starts, attempt every settlement and preserve all failure evidence.
    if !control.recovering {
        if let Some(reason) = &reason {
            for result in [
                store.fail_chat_slice_worker(control, &request.slice_key, reason),
                store.settle_chat_build_board(control, &request.slice_key, Some(reason)),
            ] {
                if let Err(error) = result {
                    problems.push(format!(
                        "recording continuation failure also failed: {error}"
                    ));
                }
            }
        }
    }
    let message = (!problems.is_empty()).then(|| problems.join("; "));
    if let Err(error) = store.finish_chat_build(control, message.as_deref()) {
        problems.push(format!("settling continuation also failed: {error}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::invalid(problems.join("; ")))
    }
}
