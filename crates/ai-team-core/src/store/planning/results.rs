//! Control-plane board settlement; models can report progress but cannot release claims.

use super::{super::chat_teams::build, engine};
use crate::{ChatBuildControl, Error, Result, Store};
use rusqlite::params;

impl Store {
    pub(crate) fn owned_chat_build_slice(
        &self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<ai_planner_core::Slice> {
        build::check(self.db().conn(), control, false)?;
        let approved = build::read(self.db().conn(), control.receipt.run_id, key)?;
        let chat = self.chat(control.receipt.chat_id)?;
        let store = engine::open(&self.planning_path()?, false)?
            .ok_or_else(|| Error::invalid("the owned planning store is missing"))?;
        let plan =
            engine::find(&store, &chat)?.ok_or_else(|| Error::invalid("this chat has no plan"))?;
        let slice = store.slice_by_id(approved.planner_slice_id)?;
        if slice.plan_id != plan.id || slice.key != key || !owned(&slice, &approved, chat.id) {
            return Err(Error::invalid(
                "the approved build no longer holds this exact slice claim",
            ));
        }
        Ok(slice)
    }

    // Lock order is deliberately team -> planner, matching every chat plan writer.
    // This is not an atomic two-DB commit: a replay rechecks the exact identity and
    // changes neither an already-set status nor an already-released claim. Its target
    // is derived from durable build/return evidence, never a model's completion claim.
    pub(crate) fn settle_chat_build_board(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let chat = self.chat(control.receipt.chat_id)?;
        let path = self.planning_path()?;
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            let approved = build::read(tx, control.receipt.run_id, key)?;
            let mut store = engine::open(&path, false)?.ok_or_else(|| Error::invalid("the owned planning store is missing"))?;
            let plan = engine::find(&store, &chat)?.ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let mut slice = store.slice_by_id(approved.planner_slice_id)?;
            if slice.plan_id != plan.id || slice.key != key || (slice.claimed_by.is_some() && !owned(&slice, &approved, chat.id)) { return Err(Error::invalid("refusing to change somebody else's slice claim")); }
            let verified = approved.build_status == "verified" && approved.commit_sha.is_some();
            if !verified && reason.is_none() { return Err(Error::invalid("unverified work needs a reason, not a completion status")); }
            let status = if verified { ai_planner_core::Status::InReview } else { ai_planner_core::Status::Blocked };
            store.set_actor(format!("ai-team chat-{} run-{}", chat.id, approved.run_id));
            if slice.status != status { slice = store.set_slice_status(&slice, status, reason)?; }
            let policy: crate::OnFailure = tx.query_row("SELECT on_failure FROM run WHERE id = ?1", [approved.run_id], |row| row.get(0))?;
            if slice.claimed_by.is_some() && ((verified && approved.lease_state == "released") || (!verified && crate::Fallout::of(policy) == crate::Fallout::AbortBranch)) { store.release_slice(&slice)?; }
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'ai-team', ?3)",
                params![approved.run_id, crate::now(), format!("{key}: {}{}", if verified { "draft ready for human review" } else { "blocked" }, reason.map_or(String::new(), |reason| format!(": {reason}")))])?;
            Ok(())
        })
    }
}

fn owned(slice: &ai_planner_core::Slice, approved: &crate::ChatBuildSlice, chat: i64) -> bool {
    slice.claimed_by.as_deref() == Some(&format!("ai-team chat-{chat} run-{}", approved.run_id))
        && slice.branch == approved.branch
        && slice
            .worktree_path
            .as_deref()
            .zip(approved.worktree_path.as_deref())
            .is_some_and(|(left, right)| crate::same_worktree(left, right))
}
