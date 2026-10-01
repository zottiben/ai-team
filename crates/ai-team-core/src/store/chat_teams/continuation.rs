use super::build;
use crate::{ChatBuildControl, ChatBuildResume, ChatBuildSlice, Error, Result, Store};
use rusqlite::params;

pub(super) fn resumable(row: &ChatBuildSlice, revision: i64) -> Result<()> {
    if row.rev != revision {
        return Err(Error::invalid(
            "the retained slice changed; refresh before continuing",
        ));
    }
    if !matches!(row.lease_state.as_str(), "retained" | "leased")
        || row.worktree_path.is_none()
        || row.lease_holder.is_none()
        || row.branch.is_none()
        || row.release_started
        || row.candidate_sha.is_some()
        || row.commit_sha.is_some()
        || row.build_status == "verified"
    {
        return Err(Error::invalid(
            "this slice needs evidence reconciliation, not a model restart",
        ));
    }
    Ok(())
}

impl Store {
    pub(crate) fn activate_chat_slice_continuation(
        &mut self,
        control: &mut ChatBuildControl,
        request: &ChatBuildResume,
    ) -> Result<ChatBuildSlice> {
        self.db_mut().write(|tx| {
            build::check(tx, control, true)?;
            if !control.recovering { return Err(Error::invalid("continuation was already activated")); }
            let row = build::read(tx, control.receipt.run_id, &request.slice_key)?;
            resumable(&row, request.expect_slice_revision)?;
            build::approved_agent(tx, &row)?;
            let stopped: bool = tx.query_row("SELECT stop_requested FROM chat WHERE id = ?1", [control.receipt.chat_id], |row| row.get(0))?;
            if stopped { return Err(Error::invalid("Stopped by you before continuation; work is kept.")); }
            tx.execute("UPDATE chat_build_slice SET lease_state = 'leased', build_status = 'running', rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2", params![row.run_id,row.slice_key])?;
            tx.execute("UPDATE run SET status = 'running', blocked_reason = NULL, ended_at = NULL, rev = rev + 1, updated_at = ?2 WHERE id = ?1", params![row.run_id,crate::now()])?;
            tx.execute("INSERT INTO event (run_id,at,kind,actor,summary) VALUES (?1,?2,'note','ai-team',?3)", params![row.run_id,crate::now(),format!("Continuing {} in its original lease; no work was reset or reacquired", row.slice_key)])?;
            build::read(tx, row.run_id, &row.slice_key)
        }).inspect(|_| { control.recovering = false; })
    }
}
