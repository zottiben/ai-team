//! Human reconciliation of drained builds; never an agent dispatch/resume authority.
use super::build;
use crate::chat::team::{ownership::Ownership, TeamControl};
use crate::{ChatBuildControl, ChatBuildRecovery, ChatTeamPhase, Error, Result, Store};
use rusqlite::params;

impl Store {
    pub(crate) fn claim_chat_build_recovery(
        &mut self,
        target: &ChatBuildRecovery,
    ) -> Result<ChatBuildControl> {
        self.claim_quiescent_chat_build(target, None, false)
    }

    pub(crate) fn claim_quiescent_chat_build(
        &mut self,
        target: &ChatBuildRecovery,
        resume: Option<&crate::ChatBuildResume>,
        closing: bool,
    ) -> Result<ChatBuildControl> {
        let ownership = Ownership::acquire(self.path(), target.run_id)?;
        let execution = self
            .chat_team_run(target.run_id)?
            .ok_or_else(|| Error::invalid("this is not a chat team execution"))?;
        if execution.chat_id != target.chat_id
            || execution.control_node_id != target.node_id
            || execution.rev != target.expect_revision
        {
            return Err(Error::invalid(
                "this is not the reviewed recovery target; refresh the chat",
            ));
        }
        if !matches!(
            execution.phase,
            ChatTeamPhase::Blocked | ChatTeamPhase::Building
        ) || !execution.quiescent
        {
            return Err(Error::invalid("this execution has not certified drained processes; interrupted-task/process recovery is required before cleanup"));
        }
        if self
            .chat_team_members(target.run_id)?
            .iter()
            .any(crate::ChatTeamMember::pi_alive)
        {
            return Err(Error::invalid(
                "a team process is still alive; do not recover over it",
            ));
        }
        let base_sha = execution
            .base_sha
            .ok_or_else(|| Error::invalid("this execution has no approved base"))?;
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("could not identify the recovery controller"))?;
        self.db_mut().write(|tx| {
            if !closing { super::closure::require_open(tx, target.run_id)?; }
            if let Some(request) = resume {
                let row = build::read(tx, target.run_id, &request.slice_key)?;
                super::continuation::resumable(&row, request.expect_slice_revision)?;
                build::approved_agent(tx, &row)?;
                // Consume the old stop in the same transaction as the exact claim.
                // Any stop arriving after this point wins over validation/dispatch.
                tx.execute("UPDATE chat SET stop_requested = 0, rev = rev + 1 WHERE id = ?1", [target.chat_id])?;
                tx.execute("INSERT INTO event (run_id,at,kind,actor,summary) VALUES (?1,?2,'note','human',?3)", params![target.run_id,crate::now(),format!("Requested continuation of retained slice {}", request.slice_key)])?;
            }
            if tx.execute("UPDATE chat_team_run SET phase = 'building', supervisor_pid = ?5, supervisor_identity = ?6, controller_protocol = 1, child_journal = 1, child_epoch = child_epoch + 1, quiescent = 0, rev = rev + 1
                WHERE run_id = ?1 AND chat_id = ?2 AND control_node_id = ?3 AND rev = ?4 AND phase IN ('building','blocked') AND quiescent = 1 AND supervisor_pid IS NULL
                AND approved_revision IS NOT NULL AND base_sha IS NOT NULL
                AND EXISTS(SELECT 1 FROM chat WHERE id = ?2 AND active_node_id = ?3 AND archived = 0)
                AND NOT EXISTS(SELECT 1 FROM node_run WHERE run_id = ?1 AND (pi_pid IS NOT NULL OR status IN ('queued','running')))",
                params![target.run_id, target.chat_id, target.node_id, target.expect_revision, pid, identity])? != 1 { return Err(Error::invalid("this recovery was already claimed or its execution changed")); }
            // Deliberately not Running: reconciliation cannot enroll a model or grant MCP access.
            tx.execute("UPDATE run SET status = 'blocked', blocked_reason = 'Reconciling recorded build evidence', rev = rev + 1, updated_at = ?2 WHERE id = ?1", params![target.run_id, crate::now()])?;
            Ok(())
        })?;
        ownership.bind(self)?;
        Ok(ChatBuildControl {
            receipt: TeamControl {
                chat_id: target.chat_id,
                run_id: target.run_id,
                node_id: target.node_id,
                revision: target.expect_revision + 1,
                owner: Some(pid),
            },
            base_sha,
            ownership,
            recovering: true,
        })
    }

    pub(crate) fn release_unacquired_chat_slice(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            recovery(tx, control)?;
            if tx.execute("UPDATE chat_build_slice SET lease_state = 'released', build_status = 'failed', reason = 'Cancelled during recovery before acquisition was attempted', rev = rev + 1
                WHERE run_id = ?1 AND slice_key = ?2 AND lease_state = 'pending' AND worktree_path IS NULL AND candidate_sha IS NULL AND maker_node_id IS NULL AND verifier_node_id IS NULL", params![control.receipt.run_id, key])? != 1 { return Err(Error::invalid("this slice already has acquisition or worker evidence; retain it")); }
            Ok(())
        })
    }

    pub(crate) fn record_recovered_lease(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        path: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            recovery(tx, control)?;
            let slice = build::read(tx, control.receipt.run_id, key)?;
            if slice.worktree_path.is_some() { return Err(Error::invalid("the acquisition already has a path; do not replace it")); }
            let mut query = tx.prepare("SELECT worktree_path FROM chat_build_slice WHERE worktree_path IS NOT NULL AND lease_state != 'released'
                UNION SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL
                UNION SELECT worktree_path FROM node_run WHERE worktree_path IS NOT NULL AND status NOT IN ('done','failed','cancelled')")?;
            let owners = query.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if owners.iter().any(|owner| crate::same_worktree(owner, path)) { return Err(Error::invalid("another execution owns this acquisition path")); }
            tx.execute("UPDATE chat_build_slice SET worktree_path = ?3, lease_state = 'retained', rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2", params![slice.run_id, key, path])?;
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'ai-team', ?3)", params![slice.run_id, crate::now(), format!("{key}: located the retained acquisition; no model started and no files reset")])?;
            Ok(())
        })
    }

    pub(crate) fn reconcile_lease_state(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        returned: bool,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            recovery(tx, control)?;
            if tx.execute("UPDATE chat_build_slice SET lease_state = ?3, reason = NULL, rev = rev + 1
                WHERE run_id = ?1 AND slice_key = ?2 AND build_status = 'verified' AND commit_sha IS NOT NULL
                AND lease_state IN ('retained','leased') AND (NOT ?4 OR release_started = 1)", params![control.receipt.run_id, key, if returned { "released" } else { "leased" }, returned])? != 1 { return Err(Error::invalid("this lease has no matching verified return evidence")); }
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'ai-team', ?3)", params![control.receipt.run_id, crate::now(), format!("{key}: reconciled {}", if returned { "the recorded return without touching the pool entry" } else { "the exact verified lease for return" })])?;
            Ok(())
        })
    }
}

fn recovery(conn: &rusqlite::Connection, control: &ChatBuildControl) -> Result<()> {
    build::check(conn, control, true)?;
    if !control.recovering {
        return Err(Error::invalid(
            "this action needs the human recovery controller",
        ));
    }
    Ok(())
}
