//! Persisted team execution identity, separate from any individual Pi turn.

mod build;
mod planning;
pub use build::{
    ChatBuildApproval, ChatBuildControl, ChatBuildReview, ChatBuildSlice, ChatBuildStart,
};
pub use planning::drive_chat_team_planning;

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
    #[serde(skip)]
    pub(crate) supervisor_pid: Option<i64>,
    #[serde(skip)]
    pub(crate) supervisor_identity: Option<String>,
}

impl ChatTeamRun {
    pub fn supervisor_alive(&self) -> bool {
        super::process_matches(self.supervisor_pid, self.supervisor_identity.as_deref())
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
