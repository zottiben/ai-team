//! The team database journals the external lease/claim protocol and approved seat facts.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::chat::team::TeamControl;
use crate::{ChatBuildControl, ChatBuildSlice, ChatBuildStart, Error, Result, Store};

const SELECT: &str = "SELECT run_id, slice_key, planner_slice_id, approved_rev, assigned_agent_id, assigned_agent_rev,
    agent_snapshot, worktree_path, lease_holder, branch, lease_state, reason, rev FROM chat_build_slice";

impl Store {
    pub fn chat_build_slices(&self, run: i64) -> Result<Vec<ChatBuildSlice>> {
        let mut stmt = self.db().conn().prepare(&format!(
            "{SELECT} WHERE run_id = ?1 ORDER BY planner_slice_id"
        ))?;
        let rows = stmt
            .query_map([run], from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn claim_chat_build(&mut self, start: &ChatBuildStart) -> Result<ChatBuildControl> {
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("could not identify the build controller"))?;
        let base = self.db_mut().write(|tx| {
            let changed = tx.execute("UPDATE chat_team_run SET supervisor_pid = ?5, supervisor_identity = ?6, rev = rev + 1
                WHERE run_id = ?1 AND chat_id = ?2 AND control_node_id = ?3 AND rev = ?4 AND supervisor_pid IS NULL
                AND phase = 'building' AND approved_revision IS NOT NULL AND base_sha IS NOT NULL
                AND EXISTS (SELECT 1 FROM chat WHERE id = ?2 AND active_node_id = ?3 AND stop_requested = 0 AND archived = 0)
                AND EXISTS (SELECT 1 FROM run WHERE id = ?1 AND status = 'running')",
                params![start.run_id, start.chat_id, start.node_id, start.revision, pid, identity])?;
            if changed != 1 { return Err(Error::invalid("the approved build was already started, stopped, or changed")); }
            Ok(tx.query_row("SELECT base_sha FROM chat_team_run WHERE run_id = ?1", [start.run_id], |row| row.get(0))?)
        })?;
        Ok(ChatBuildControl {
            receipt: TeamControl {
                chat_id: start.chat_id,
                run_id: start.run_id,
                node_id: start.node_id,
                revision: start.revision + 1,
                owner: Some(pid),
            },
            base_sha: base,
        })
    }

    pub(crate) fn chat_build_agent(
        &self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<crate::Agent> {
        check(self.db().conn(), control, false)?;
        approved_agent(
            self.db().conn(),
            &read(self.db().conn(), control.receipt.run_id, key)?,
        )
    }

    pub(crate) fn reserve_chat_build_lease(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
    ) -> Result<ChatBuildSlice> {
        self.db_mut().write(|tx| {
            check(tx, control, false)?;
            let slice = read(tx, control.receipt.run_id, key)?;
            let agent = approved_agent(tx, &slice)?;
            if !agent.enabled || agent.read_only { return Err(Error::invalid("the approved maker is no longer enabled for writing")); }
            let (head, dirty) = super::super::planning::build::checkout(std::path::Path::new(&tx.query_row::<String, _, _>("SELECT workspace_path FROM chat WHERE id = ?1", [control.receipt.chat_id], |row| row.get(0))?))?;
            if head != control.base_sha || !dirty.is_empty() { return Err(Error::invalid("the approved checkout changed; preserve its files and review a new build before leasing")); }
            if tx.execute("UPDATE chat_build_slice SET lease_state = 'acquiring', rev = rev + 1 WHERE run_id = ?1 AND slice_key = ?2 AND lease_state = 'pending'",
                params![slice.run_id, slice.slice_key])? != 1 {
                return Err(Error::invalid("this slice already has acquisition or recovery evidence; do not take a second lease"));
            }
            read(tx, slice.run_id, key)
        })
    }

    pub(crate) fn record_chat_build_lease(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        path: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, control, false)?;
            // Check all physical owners, including parked leases and legacy nodes.
            let mut stmt = tx.prepare("SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL
                UNION SELECT worktree_path FROM node_run WHERE worktree_path IS NOT NULL AND status NOT IN ('done','failed','cancelled')
                UNION SELECT worktree_path FROM chat_build_slice WHERE worktree_path IS NOT NULL AND lease_state != 'released'")?;
            let owners = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if owners.iter().any(|owner| crate::same_worktree(owner, path)) { return Err(Error::invalid("another execution or chat owns the returned worktree")); }
            if tx.execute("UPDATE chat_build_slice SET worktree_path = ?3, lease_state = 'claiming', rev = rev + 1
                WHERE run_id = ?1 AND slice_key = ?2 AND lease_state = 'acquiring'", params![control.receipt.run_id, key, path])? != 1 {
                return Err(Error::invalid("this acquisition no longer belongs to the controller"));
            }
            Ok(())
        })
    }

    pub(crate) fn retain_chat_build_lease(
        &mut self,
        control: &ChatBuildControl,
        key: &str,
        returned_path: Option<&str>,
        reason: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            check(tx, control, true)?;
            if tx.execute("UPDATE chat_build_slice SET lease_state = 'retained', worktree_path = COALESCE(worktree_path, ?3), reason = ?4, rev = rev + 1
                WHERE run_id = ?1 AND slice_key = ?2 AND lease_state IN ('acquiring','claiming','leased','retained')",
                params![control.receipt.run_id, key, returned_path, reason])? != 1 { return Err(Error::invalid("this slice has no acquisition to retain")); }
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'failed', 'ai-team', ?3)",
                params![control.receipt.run_id, crate::now(), format!("{key}: retained acquisition/lease for recovery: {reason}")])?;
            Ok(())
        })
    }

    /// Cancellation before any external operation is safe and needs no worktree cleanup.
    pub fn cancel_unstarted_chat_build(&mut self, start: &ChatBuildStart) -> Result<()> {
        self.db_mut().write(|tx| {
            let changed = tx.execute("UPDATE chat_team_run SET phase = 'finished', reason = 'Cancelled before dispatch', rev = rev + 1
                WHERE run_id = ?1 AND chat_id = ?2 AND control_node_id = ?3 AND rev = ?4 AND phase = 'building' AND supervisor_pid IS NULL
                AND EXISTS(SELECT 1 FROM chat WHERE id = ?2 AND active_node_id = ?3)
                AND NOT EXISTS(SELECT 1 FROM chat_build_slice WHERE run_id = ?1 AND lease_state != 'pending')",
                params![start.run_id, start.chat_id, start.node_id, start.revision])?;
            if changed != 1 { return Err(Error::invalid("this build needs its active controller or lease-aware recovery")); }
            tx.execute("UPDATE chat_build_slice SET lease_state = 'released', rev = rev + 1 WHERE run_id = ?1", [start.run_id])?;
            tx.execute("UPDATE run SET status = 'cancelled', ended_at = ?2, updated_at = ?2, rev = rev + 1 WHERE id = ?1", params![start.run_id, crate::now()])?;
            tx.execute("UPDATE chat SET active_node_id = NULL, stop_requested = 0, rev = rev + 1 WHERE id = ?1", [start.chat_id])?;
            tx.execute("INSERT INTO event (run_id, at, kind, actor, summary) VALUES (?1, ?2, 'note', 'human', 'Cancelled the approved build before dispatch')", params![start.run_id, crate::now()])?;
            Ok(())
        })
    }
}

pub(in crate::store) fn check(
    conn: &Connection,
    control: &ChatBuildControl,
    stopping: bool,
) -> Result<()> {
    let c = control.receipt;
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_team_run t JOIN chat c ON c.id = t.chat_id JOIN run r ON r.id = t.run_id
        WHERE t.run_id = ?1 AND t.chat_id = ?2 AND t.control_node_id = ?3 AND t.rev = ?4 AND t.supervisor_pid IS ?5 AND t.base_sha = ?6
        AND t.phase = 'building' AND c.active_node_id = ?3 AND c.archived = 0 AND (?7 OR (c.stop_requested = 0 AND r.status = 'running')))",
        params![c.run_id, c.chat_id, c.node_id, c.revision, c.owner, control.base_sha, stopping], |row| row.get(0))?;
    if !valid {
        return Err(Error::invalid(
            "this build controller no longer owns the active approved execution",
        ));
    }
    Ok(())
}

pub(in crate::store) fn read(conn: &Connection, run: i64, key: &str) -> Result<ChatBuildSlice> {
    conn.query_row(
        &format!("{SELECT} WHERE run_id = ?1 AND slice_key = ?2"),
        params![run, key],
        from_row,
    )
    .optional()?
    .ok_or_else(|| Error::invalid("that slice was not approved in this execution"))
}

pub(in crate::store) fn approved_agent(
    conn: &Connection,
    slice: &ChatBuildSlice,
) -> Result<crate::Agent> {
    let snapshot: crate::Agent = serde_json::from_str(&slice.agent_snapshot)
        .map_err(|_| Error::invalid("this slice has no valid approved maker snapshot"))?;
    let fresh = super::super::agents::agents_in(conn, snapshot.team_id)?
        .into_iter()
        .find(|agent| Some(agent.id) == slice.assigned_agent_id)
        .ok_or_else(|| Error::invalid("the approved maker no longer exists"))?;
    if Some(fresh.rev) != slice.assigned_agent_rev
        || serde_json::to_value(&fresh)? != serde_json::to_value(&snapshot)?
    {
        return Err(Error::invalid(
            "the approved maker changed; review a new build rather than inheriting stale approval",
        ));
    }
    Ok(fresh)
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<ChatBuildSlice> {
    Ok(ChatBuildSlice {
        run_id: row.get(0)?,
        slice_key: row.get(1)?,
        planner_slice_id: row.get(2)?,
        approved_rev: row.get(3)?,
        assigned_agent_id: row.get(4)?,
        assigned_agent_rev: row.get(5)?,
        agent_snapshot: row.get(6)?,
        worktree_path: row.get(7)?,
        lease_holder: row.get(8)?,
        branch: row.get(9)?,
        lease_state: row.get(10)?,
        reason: row.get(11)?,
        rev: row.get(12)?,
    })
}
