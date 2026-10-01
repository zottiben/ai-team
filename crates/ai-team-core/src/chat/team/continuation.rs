//! Explicit human continuation is neither process recovery nor another build approval.

use crate::{ChatBuildRecovery, Error, Result, Store};
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
    let mut store = Store::open(db)?;
    let mut control = store.claim_quiescent_chat_build(&request.target, Some(request), false)?;
    let outcome = control
        .ownership
        .clone()
        .track(super::execution::resume(db, &mut control, request))
        .await;
    let reason = outcome.as_ref().err().map(ToString::to_string);
    let mut problems: Vec<_> = reason.iter().cloned().collect();
    // Refused validation must not mutate the slice or release its claim. Once a
    // worker starts, attempt every settlement and preserve all failure evidence.
    if !control.recovering {
        if let Some(reason) = &reason {
            for result in [
                store.fail_chat_slice_worker(&control, &request.slice_key, reason),
                store.settle_chat_build_board(&control, &request.slice_key, Some(reason)),
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
    if let Err(error) = store.finish_chat_build(&mut control, message.as_deref()) {
        problems.push(format!("settling continuation also failed: {error}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::invalid(problems.join("; ")))
    }
}
