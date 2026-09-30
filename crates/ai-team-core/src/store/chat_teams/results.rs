//! Execution receipts and immutable delivery evidence for the approved build controller.

use super::build;
use crate::{ChatBuildControl, ChatBuildSlice, Error, NodeStatus, Result, Store};
use rusqlite::params;

impl Store {
    pub(crate) fn check_chat_build_cleanup(&self, control: &ChatBuildControl) -> Result<()> {
        build::check(self.db().conn(), control, true)
    }

    pub(crate) fn check_chat_build(&self, control: &ChatBuildControl) -> Result<()> {
        build::check(self.db().conn(), control, false)
    }

    pub(crate) fn begin_chat_slice_worker(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<ChatBuildSlice> {
        self.db_mut().write(|tx| {
            build::check(tx, control, false)?;
            let changed = tx.execute("UPDATE chat_build_slice SET build_status = 'running', rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2 AND lease_state = 'leased' AND build_status = 'pending'", params![control.receipt.run_id, key])?;
            if changed != 1 { return Err(Error::invalid("this approved slice already started or needs recovery")); }
            build::read(tx, control.receipt.run_id, key)
        })
    }

    pub(crate) fn mark_chat_build_attempt(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        node: i64,
        reader: bool,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            build::check(tx, control, false)?;
            let changed = tx.execute("UPDATE chat_build_slice SET maker_node_id = CASE WHEN ?4 THEN maker_node_id ELSE ?3 END,
                verifier_node_id = CASE WHEN ?4 THEN ?3 ELSE NULL END, rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2 AND build_status = 'running'
                AND EXISTS(SELECT 1 FROM node_run n JOIN chat_team_node m ON m.node_id = n.id WHERE n.id = ?3 AND n.run_id = ?1 AND n.slice_key = ?2 AND m.plan_access = CASE WHEN ?4 THEN 'reader' ELSE 'maker' END)",
                params![control.receipt.run_id, key, node, reader])?;
            if changed != 1 { return Err(Error::invalid("this attempt does not belong to the approved slice worker")); }
            Ok(())
        })
    }

    pub(crate) fn inherit_chat_build_session(
        &mut self,
        control: &ChatBuildControl,
        previous: i64,
        next: i64,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            build::check(tx, control, false)?;
            let changed = tx.execute("UPDATE node_run AS n SET session_id = p.session_id, stream_cursor = p.stream_cursor, context_tokens = p.context_tokens,
                rev = n.rev + 1, updated_at = ?4 FROM node_run p WHERE n.id = ?1 AND p.id = ?2 AND n.run_id = ?3 AND p.run_id = ?3
                AND n.agent_id = p.agent_id AND n.slice_key = p.slice_key AND n.provider = p.provider AND n.model = p.model
                AND n.status = 'queued' AND p.status NOT IN ('queued','running') AND p.session_retired_at IS NULL AND p.session_id IS NOT NULL",
                params![next, previous, control.receipt.run_id, crate::now()])?;
            if changed != 1 { return Err(Error::invalid("the previous maker session cannot be inherited by this attempt")); }
            Ok(())
        })
    }

    pub(crate) fn settle_chat_build_member(
        &mut self,
        control: &ChatBuildControl,
        node: i64,
        status: NodeStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            NodeStatus::Done | NodeStatus::Failed | NodeStatus::Cancelled
        ) {
            return Err(Error::invalid("a build attempt must settle terminally"));
        }
        let member = self
            .chat_team_members(control.receipt.run_id)?
            .into_iter()
            .find(|member| member.node.id == node && member.node.slice_key.is_some())
            .ok_or_else(|| Error::invalid("this node is not a worker in the approved build"))?;
        if member.pi_alive() {
            return Err(Error::invalid("reap the worker before settling it"));
        }
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            tx.execute("UPDATE node_run SET status = ?2, blocked_reason = ?3, pi_pid = NULL, supervisor_pid = NULL, ended_at = ?4, updated_at = ?4, rev = rev + 1 WHERE id = ?1", params![node, status, reason, crate::now()])?;
            tx.execute("UPDATE chat_team_node SET pi_identity = NULL, live_text = '' WHERE node_id = ?1", [node])?;
            Ok(())
        })
    }

    pub(crate) fn record_chat_candidate(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        sha: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            build::check(tx, control, false)?;
            if tx.execute("UPDATE chat_build_slice SET candidate_sha = ?3, rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2 AND build_status = 'running' AND commit_sha IS NULL", params![control.receipt.run_id, key, sha])? != 1 { return Err(Error::invalid("this worker cannot record a candidate")); }
            Ok(())
        })
    }

    pub(crate) fn record_chat_commit(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        sha: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            if tx.execute("UPDATE chat_build_slice SET commit_sha = ?3, build_status = 'verified', rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2 AND (build_status = 'running' OR ?4) AND commit_sha IS NULL AND candidate_sha = ?3", params![control.receipt.run_id, key, sha, control.recovering])? != 1 { return Err(Error::invalid("this candidate no longer belongs to the worker")); }
            tx.execute("INSERT INTO review (project_id, run_id, node_run_id, title, branch, base_sha, head_sha, created_at, updated_at)
                SELECT c.project_id, s.run_id, s.maker_node_id, s.slice_key || ': verified draft', s.branch, t.base_sha, s.commit_sha, ?3, ?3
                FROM chat_build_slice s JOIN chat_team_run t ON t.run_id = s.run_id JOIN chat c ON c.id = t.chat_id
                WHERE s.run_id = ?1 AND s.slice_key = ?2", params![control.receipt.run_id, key, crate::now()])?;
            tx.execute("INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                SELECT run_id, maker_node_id, ?3, 'note', 'ai-team', ?4, ?5 FROM chat_build_slice WHERE run_id = ?1 AND slice_key = ?2",
                params![control.receipt.run_id, key, crate::now(), format!("{key}: verified draft committed; not merged or published"), serde_json::json!({"slice":key,"commit":sha}).to_string()])?;
            Ok(())
        })
    }

    pub(crate) fn fail_chat_slice_worker(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        reason: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            let stopped: bool = tx.query_row("SELECT stop_requested FROM chat WHERE id = ?1", [control.receipt.chat_id], |row| row.get(0))?;
            tx.execute("UPDATE chat_build_slice SET build_status = CASE WHEN build_status = 'verified' THEN build_status ELSE ?3 END,
                lease_state = CASE WHEN lease_state = 'pending' THEN 'released' WHEN lease_state = 'released' THEN lease_state ELSE 'retained' END,
                reason = ?4, rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2",
                params![control.receipt.run_id, key, if stopped { "stopped" } else { "failed" }, reason])?;
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'failed', 'ai-team', ?3)", params![control.receipt.run_id, crate::now(), format!("{key}: {reason}")])?;
            Ok(())
        })
    }

    pub(crate) fn mark_chat_lease_return(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        returned: bool,
    ) -> Result<()> {
        if self
            .chat_team_members(control.receipt.run_id)?
            .iter()
            .any(|member| {
                member.node.slice_key.as_deref() == Some(key)
                    && (member.pi_alive()
                        || matches!(member.node.status, NodeStatus::Queued | NodeStatus::Running))
            })
        {
            return Err(Error::invalid("a worker still owns this lease"));
        }
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            if tx.execute("UPDATE chat_build_slice SET release_started = 1, lease_state = CASE WHEN ?3 THEN 'released' ELSE lease_state END, rev = rev + 1
                WHERE run_id = ?1 AND slice_key = ?2 AND build_status = 'verified' AND commit_sha IS NOT NULL AND lease_state = 'leased'", params![control.receipt.run_id, key, returned])? != 1 { return Err(Error::invalid("only a verified committed lease can be returned automatically")); }
            Ok(())
        })
    }

    pub(crate) fn finish_chat_build(
        &mut self,
        control: &mut ChatBuildControl,
        reason: Option<&str>,
    ) -> Result<()> {
        let members = self.chat_team_members(control.receipt.run_id)?;
        if members.iter().any(crate::ChatTeamMember::pi_alive) {
            return Err(Error::invalid(
                "team processes remain alive; keep ownership until recovery reaps them",
            ));
        }
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            let (retained, failed): (bool, bool) = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice WHERE run_id = ?1 AND lease_state != 'released'), EXISTS(SELECT 1 FROM chat_build_slice WHERE run_id = ?1 AND build_status != 'verified')", [control.receipt.run_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let stopped: bool = tx.query_row("SELECT stop_requested FROM chat WHERE id = ?1", [control.receipt.chat_id], |row| row.get(0))?;
            let undrained: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_child WHERE run_id = ?1 AND state != 'drained')", [control.receipt.run_id], |row| row.get(0))?;
            let quiescent = control.ownership.quiescent() && !undrained;
            let blocked = retained || reason.is_some() || !quiescent;
            let status = if blocked { "blocked" } else if stopped { "cancelled" } else if failed { "failed" } else { "done" };
            let message = reason.unwrap_or(if retained { "Work retained for explicit recovery; no lease was silently discarded." } else if failed { "Some approved work did not finish." } else { "Verified drafts are ready for review; nothing was merged or published." });
            let at = crate::now();
            tx.execute("UPDATE node_run SET status = 'failed', blocked_reason = ?2, pi_pid = NULL, supervisor_pid = NULL, ended_at = ?3, updated_at = ?3, rev = rev + 1 WHERE run_id = ?1 AND status IN ('queued','running')", params![control.receipt.run_id, message, at])?;
            tx.execute("UPDATE chat_team_run SET phase = ?2, reason = ?3, supervisor_pid = NULL, supervisor_identity = NULL, quiescent = ?4, rev = rev + 1 WHERE run_id = ?1", params![control.receipt.run_id, if blocked { "blocked" } else { "finished" }, message, quiescent])?;
            tx.execute("UPDATE run SET status = ?2, blocked_reason = ?3, ended_at = ?4, updated_at = ?5, rev = rev + 1 WHERE id = ?1", params![control.receipt.run_id, status, (status != "done").then_some(message), (!blocked).then_some(&at), at])?;
            tx.execute("UPDATE chat SET active_node_id = CASE WHEN ?2 THEN active_node_id ELSE NULL END, stop_requested = 0, live_text = '', supervisor_identity = NULL, pi_identity = NULL, rev = rev + 1, updated_at = ?3 WHERE id = ?1", params![control.receipt.chat_id, blocked, at])?;
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'ai-team', ?3)", params![control.receipt.run_id, at, message])?;
            Ok(())
        })?;
        control.receipt.revision += 1;
        control.receipt.owner = None;
        Ok(())
    }
}
