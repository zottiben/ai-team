//! Closing withdraws execution approval, not responsibility for kept files or leases.
use crate::{ChatBuildRecovery, ChatBuildSlice, ChatTeamRun, Error, Result, Store};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatBuildClose {
    pub target: ChatBuildRecovery,
    pub expect_plan_revision: i64,
    pub expect_slices: BTreeMap<String, i64>,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatKeptPath {
    pub chat_id: i64,
    pub run_id: i64,
    pub slice_key: String,
    pub expect_slice_revision: i64,
    pub worktree: std::path::PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatBuildClosure {
    pub run_id: i64,
    pub reason: String,
    pub requested_at: String,
    pub finished_at: Option<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatBuildCloseReport {
    pub execution: ChatTeamRun,
    pub closure: ChatBuildClosure,
    /// All approved slices, including never-acquired or already-released rows.
    pub slices: Vec<ChatBuildSlice>,
}

#[derive(Debug, Serialize)]
pub struct ChatRetainedBuild {
    pub execution: ChatTeamRun,
    pub closure: Option<ChatBuildClosure>,
    /// Recorded responsibility, not proof this pool entry is still ours. No lease
    /// inspection or return is performed by this read surface.
    pub slices: Vec<ChatBuildSlice>,
}

/// Human-only terminal exit for an exactly reviewed, drained build. Keep every
/// uncertain acquisition and every leased/staged/ignored file; invoke no Git/awt/Pi.
/// This permanently withdraws the run's approval, even if board settlement fails.
pub fn close_chat_team_build(db: &Path, request: &ChatBuildClose) -> Result<ChatBuildCloseReport> {
    if request.reason.trim().is_empty() || request.reason.len() > 4000 {
        return Err(Error::invalid(
            "give a nonempty closure reason of at most 4000 bytes",
        ));
    }
    let mut store = Store::open(db)?;
    let mut control = store.claim_quiescent_chat_build(&request.target, None, true)?;
    let result = store
        .begin_chat_build_close(&control, request)
        .and_then(|()| store.settle_chat_build_close(&mut control, request));
    if let Err(error) = result {
        let reason = error.to_string();
        store
            .finish_chat_build(&mut control, Some(&reason))
            .map_err(|cleanup| {
                Error::invalid(format!(
                    "{reason}; parking the incomplete closure also failed: {cleanup}"
                ))
            })?;
        return Err(error);
    }
    Ok(ChatBuildCloseReport {
        execution: store
            .chat_team_run(request.target.run_id)?
            .ok_or_else(|| Error::invalid("the closed execution disappeared"))?,
        closure: store
            .chat_build_closure(request.target.run_id)?
            .ok_or_else(|| Error::invalid("the closure evidence disappeared"))?,
        slices: store.chat_build_slices(request.target.run_id)?,
    })
}
