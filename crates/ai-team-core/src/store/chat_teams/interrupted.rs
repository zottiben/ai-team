use crate::chat::team::ownership::Ownership;
use crate::{ChatBuildRecovery, ChatTeamRun, Error, Result, Store};
use rusqlite::params;
use std::sync::Arc;

impl Store {
    pub(crate) fn claim_interrupted_team(
        &mut self,
        target: &ChatBuildRecovery,
    ) -> Result<Arc<Ownership>> {
        let owner = Ownership::acquire(self.path(), target.run_id)?;
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("cannot identify recovery controller"))?;
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_team_run SET phase = 'blocked', supervisor_pid = ?5, supervisor_identity = ?6, child_epoch = child_epoch + 1, rev = rev + 1
                WHERE run_id = ?1 AND chat_id = ?2 AND control_node_id = ?3 AND rev = ?4 AND child_journal = 1 AND controller_protocol = 1 AND quiescent = 0
                AND phase IN ('grounding','planning','building','blocked')
                AND EXISTS(SELECT 1 FROM chat WHERE id = ?2 AND active_node_id = ?3 AND archived = 0)", params![target.run_id,target.chat_id,target.node_id,target.expect_revision,pid,identity])? != 1 { return Err(Error::invalid("this is not the exact interrupted journalled execution; refresh or use quiescent reconciliation")); }
            tx.execute("UPDATE run SET status = 'blocked', blocked_reason = 'Stopping interrupted team processes', rev = rev + 1, updated_at = ?2 WHERE id = ?1", params![target.run_id,crate::now()])?;
            Ok(())
        })?;
        owner.bind(self)?;
        Ok(owner)
    }

    pub(crate) fn finish_interrupted_team(
        &mut self,
        owner: &Ownership,
        problem: Option<&str>,
    ) -> Result<ChatTeamRun> {
        self.db_mut().write(|tx| {
            super::children::check(tx, owner)?;
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_child WHERE run_id = ?1 AND state != 'drained')", [owner.run], |row| row.get(0))?;
            let drained = !pending && problem.is_none();
            let reason = problem.unwrap_or(if drained { "Interrupted processes stopped; files, leases and sessions are kept for explicit recovery. No work was resumed." } else { "Child process evidence is incomplete; keep ownership for inspection." });
            if drained {
                tx.execute("UPDATE node_run SET status = 'failed', blocked_reason = ?2, pi_pid = NULL, supervisor_pid = NULL, ended_at = ?3, updated_at = ?3, rev = rev + 1 WHERE run_id = ?1 AND status IN ('queued','running')", params![owner.run,reason,crate::now()])?;
                tx.execute("UPDATE node_run SET pi_pid = NULL, supervisor_pid = NULL WHERE run_id = ?1", [owner.run])?;
                tx.execute("UPDATE chat_team_node SET pi_identity = NULL, live_text = '' WHERE run_id = ?1", [owner.run])?;
                tx.execute("UPDATE chat SET live_text = '', pi_identity = NULL, supervisor_identity = NULL, rev = rev + 1, updated_at = ?2 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = ?1)", params![owner.run,crate::now()])?;
            }
            tx.execute("UPDATE chat_team_run SET supervisor_pid = NULL, supervisor_identity = NULL, quiescent = ?2, reason = ?3, rev = rev + 1 WHERE run_id = ?1", params![owner.run, drained, reason])?;
            tx.execute("UPDATE run SET blocked_reason = ?2, updated_at = ?3, rev = rev + 1 WHERE id = ?1", params![owner.run, reason, crate::now()])?;
            tx.execute("INSERT INTO event (run_id,at,kind,actor,summary) VALUES (?1,?2,'note','ai-team',?3)", params![owner.run,crate::now(),reason])?;
            Ok(())
        })?;
        self.chat_team_run(owner.run)?
            .ok_or_else(|| Error::invalid("the execution disappeared"))
    }
}
