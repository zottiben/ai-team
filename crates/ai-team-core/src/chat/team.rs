//! Persisted team execution identity, separate from any individual Pi turn.

use serde::Serialize;

use crate::{ChatTeamPhase, NodeRun};

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
