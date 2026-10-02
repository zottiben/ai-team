//! Persisted team execution identity, separate from any individual Pi turn.

mod build;
pub(crate) mod children;
mod closure;
mod continuation;
mod execution;
mod interrupted;
pub(crate) mod ownership;
mod planning;
mod recovery;
mod startup;
pub use build::{
    ChatBuildApproval, ChatBuildControl, ChatBuildReview, ChatBuildSlice, ChatBuildStart,
};
pub use closure::{
    close_chat_team_build, ChatBuildClose, ChatBuildCloseReport, ChatBuildClosure, ChatKeptPath,
    ChatRetainedBuild,
};
pub use continuation::{resume_chat_team_slice, ChatBuildContinuation, ChatBuildResume};
pub use execution::drive_chat_team_build;
pub use interrupted::recover_chat_team_processes;
pub use planning::drive_chat_team_planning;
pub use recovery::{reconcile_chat_team_build, ChatBuildRecovery, ChatBuildRecoveryReport};
pub use startup::{
    recover_abandoned_chat_team, recover_abandoned_chat_teams, ChatRecoveryEntry, ChatRecoveryState,
};

use serde::Serialize;

use crate::{ChatTeamPhase, NodeRun};

/// A task-local CAS receipt. A phase transition advances it; old callbacks (even in
/// the same desktop PID) cannot settle or release a newer controller's execution.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TeamControl {
    pub chat_id: i64,
    pub run_id: i64,
    pub node_id: i64,
    pub revision: i64,
    pub owner: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTeamRun {
    pub run_id: i64,
    pub chat_id: i64,
    pub control_node_id: i64,
    pub phase: ChatTeamPhase,
    pub base_sha: Option<String>,
    pub approved_revision: Option<i64>,
    pub reason: Option<String>,
    pub rev: i64,
    pub quiescent: bool,
    #[serde(skip)]
    pub(crate) controller_lock: Option<std::path::PathBuf>,
    #[serde(skip)]
    pub(crate) supervisor_pid: Option<i64>,
    #[serde(skip)]
    pub(crate) supervisor_identity: Option<String>,
}

impl ChatTeamRun {
    pub fn supervisor_alive(&self) -> bool {
        if self.supervisor_pid.is_none() {
            return false;
        }
        self.controller_lock.as_ref().map_or_else(
            || super::process_matches(self.supervisor_pid, self.supervisor_identity.as_deref()),
            |path| ownership::held(path).unwrap_or(true),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTeamMember {
    pub node: NodeRun,
    pub live_text: String,
    #[serde(skip)]
    pub(crate) pi_identity: Option<String>,
}

impl ChatTeamMember {
    pub fn pi_alive(&self) -> bool {
        super::process_matches(self.node.pi_pid, self.pi_identity.as_deref())
    }
}
