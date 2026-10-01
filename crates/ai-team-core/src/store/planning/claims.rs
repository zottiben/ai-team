//! Claim only journaled approved work. The engine remains the authority for the claim.

use rusqlite::params;

use crate::{ChatBuildControl, Error, Result, Store};

use super::super::chat_teams::build;
use super::engine;

impl Store {
    /// Called after awt membership and branch preparation. It is safe to retry after
    /// the engine committed but the team DB failed: recognize only this exact claim.
    pub fn claim_chat_build_slice(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<ai_planner_core::Slice> {
        let chat = self.chat(control.receipt.chat_id)?;
        let path = self.planning_path()?;
        self.db_mut().write(|tx| {
            build::check(tx, control, false)?;
            let approved = build::read(tx, control.receipt.run_id, key)?;
            build::approved_agent(tx, &approved)?;
            if !matches!(approved.lease_state.as_str(), "claiming" | "retained" | "leased") {
                return Err(Error::invalid("record the returned lease before claiming its slice"));
            }
            let lease = approved.worktree_path.as_deref().ok_or_else(|| Error::invalid("this acquisition has no recorded worktree"))?;
            let branch = approved.branch.as_deref().ok_or_else(|| Error::invalid("this approval has no draft branch"))?;
            if approved.lease_state != "leased" {
                let (head, dirty) = super::build::checkout(std::path::Path::new(&chat.workspace_path))?;
                if head != control.base_sha || !dirty.is_empty() {
                    return Err(Error::invalid("the approved checkout changed during leasing; keep its files and inspect the retained lease"));
                }
                super::build::prepared(std::path::Path::new(lease), &control.base_sha, branch)?;
            }
            let mut store = engine::open(&path, false)?.ok_or_else(|| Error::invalid("the owned planning store is missing"))?;
            let plan = engine::find(&store, &chat)?.ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let slice = store.slice_by_id(approved.planner_slice_id)?;
            if slice.plan_id != plan.id || slice.key != approved.slice_key {
                return Err(Error::invalid("the approved slice no longer belongs to this chat's plan"));
            }
            let actor = format!("ai-team chat-{} run-{}", chat.id, approved.run_id);
            let owned = slice.claimed_by.as_deref() == Some(&actor)
                && slice.worktree_path.as_deref().is_some_and(|path| crate::same_worktree(path, lease))
                && slice.branch.as_deref() == Some(branch);
            let slice = if owned && (approved.lease_state == "leased" || (slice.rev == approved.approved_rev + 1 && slice.status == ai_planner_core::Status::Active)) {
                slice
            } else {
                if approved.lease_state == "retained" || slice.rev != approved.approved_rev || slice.status != ai_planner_core::Status::Ready || slice.claimed_by.is_some() || slice.branch.is_some() {
                    return Err(Error::invalid("the approved slice changed or is held elsewhere; do not steal or rewrite its claim"));
                }
                store.set_actor(actor);
                store.claim_slice(&slice, lease, Some(branch))?
            };
            if approved.lease_state != "leased" {
                tx.execute("UPDATE chat_build_slice SET lease_state = 'leased', reason = NULL, rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2",
                    params![approved.run_id, approved.slice_key])?;
                tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'ai-team', ?3)",
                    params![approved.run_id, crate::now(), format!("{} claimed in its approved worktree", approved.slice_key)])?;
            }
            Ok(slice)
        })
    }
}
