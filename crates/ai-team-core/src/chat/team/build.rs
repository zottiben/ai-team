//! Human approval and durable preparation addresses, not another copy of the work graph.

use serde::{Deserialize, Serialize};

use super::TeamControl;
use crate::planning::ChatPlan;
use crate::ChatTeamRun;

#[derive(Debug, Clone, Serialize)]
pub struct ChatBuildReview {
    pub execution: ChatTeamRun,
    pub plan: ChatPlan,
    pub roster: Vec<crate::Agent>,
    pub roster_revision: String,
    pub head: String,
    /// Git porcelain, for an explicit commit/stash decision. Never changed by approval.
    pub dirty: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatBuildApproval {
    pub expect_control_revision: i64,
    pub expect_plan_revision: i64,
    pub expect_roster_revision: String,
    pub expect_head: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatBuildStart {
    pub chat_id: i64,
    pub node_id: i64,
    pub run_id: i64,
    pub revision: i64,
}

/// Rust-only authority. No deserialization and no MCP tool that can mint one.
#[derive(Debug, Clone)]
pub struct ChatBuildControl {
    pub(crate) receipt: TeamControl,
    pub(crate) base_sha: String,
}

impl crate::Store {
    /// Prepare only an approved slice, leaving both the lease and its evidence intact
    /// on failure. The owning build controller must later run/verify or recover it.
    pub async fn prepare_chat_build_slice(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        registry: &crate::ModelRegistry,
    ) -> crate::Result<ai_planner_core::Slice> {
        if let Some(limit) = crate::run_may_continue(self, control.receipt.run_id)? {
            return Err(crate::Error::invalid(limit.reason));
        }
        registry.resolve(&self.chat_build_agent(control, key)?)?;
        let chat = self.chat(control.receipt.chat_id)?;
        let repo = self
            .project_repos(chat.project_id)?
            .into_iter()
            .find_map(|repo| repo.main_path)
            .ok_or_else(|| crate::Error::invalid("this project has no checkout"))?;
        let worktrees = crate::Worktrees::at(repo);
        worktrees
            .resolve(std::path::Path::new(&chat.workspace_path))
            .await?;
        let approved = self.reserve_chat_build_lease(control, key)?;
        let mut returned = None;
        let result = async {
            let lease = worktrees.lease(approved.lease_holder.as_deref().ok_or_else(|| crate::Error::invalid("this approval has no lease holder"))?).await?;
            let path = lease.path().to_path_buf();
            // Lease::Drop calls awt return --force. From here on even errors/panics must
            // preserve the checkout; acquisition intent already names its holder.
            lease.preserve();
            returned = Some(path.clone());
            let path = worktrees.resolve(&path).await?;
            self.record_chat_build_lease(control, key, &path.to_string_lossy())?;
            if !crate::neighbours::git::porcelain(&path).await?.is_empty() {
                return Err(crate::Error::invalid("awt returned a dirty worktree; keep it for inspection rather than resetting it"));
            }
            crate::neighbours::git::prepare_new_branch(&path, approved.branch.as_deref().ok_or_else(|| crate::Error::invalid("this approval has no draft branch"))?, &control.base_sha).await?;
            self.claim_chat_build_slice(control, key)
        }.await;
        if let Err(error) = &result {
            self.retain_chat_build_lease(
                control,
                key,
                returned
                    .as_ref()
                    .map(|path| path.to_string_lossy())
                    .as_deref(),
                &error.to_string(),
            )
            .map_err(|record| {
                crate::Error::invalid(format!(
                    "{error}; recording the retained lease also failed: {record}"
                ))
            })?;
        }
        result
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatBuildSlice {
    pub run_id: i64,
    pub slice_key: String,
    pub planner_slice_id: i64,
    pub approved_rev: i64,
    pub assigned_agent_id: Option<i64>,
    pub assigned_agent_rev: Option<i64>,
    #[serde(skip)]
    pub(crate) agent_snapshot: String,
    pub worktree_path: Option<String>,
    pub lease_holder: Option<String>,
    pub branch: Option<String>,
    pub lease_state: String,
    pub reason: Option<String>,
    pub rev: i64,
}
