//! Journal before touching the board. A failed second database commit is replayed,
//! never mistaken for permission to restart a maker or return a kept lease.
use super::{engine, results::owned};
use crate::store::chat_teams::build;
use crate::{ChatBuildClose, ChatBuildControl, ChatBuildSlice, Error, Result, Store};
use rusqlite::{params, Connection, OptionalExtension};

impl Store {
    pub(crate) fn begin_chat_build_close(
        &mut self,
        control: &ChatBuildControl,
        request: &ChatBuildClose,
    ) -> Result<()> {
        let chat = self.chat(control.receipt.chat_id)?;
        let path = self.planning_path()?;
        self.db_mut().write(|tx| {
            check(tx, control)?;
            let rows = reviewed(tx, control, request)?;
            let planner = engine::open(&path, false)?.ok_or_else(|| Error::invalid("the owned planning store is missing"))?;
            let before = super::snapshot(&planner, &chat)?;
            if before.revision != request.expect_plan_revision { return Err(Error::invalid("the reviewed plan changed; refresh before closing")); }
            let plan = before.bundle.ok_or_else(|| Error::invalid("this chat has no plan"))?.plan;
            let existing: Option<String> = tx.query_row("SELECT reason FROM chat_build_closure WHERE run_id = ?1", [control.receipt.run_id], |row| row.get(0)).optional()?;
            for row in &rows {
                if !matches_record(&planner, plan.id, chat.id, row)? && existing.is_none() {
                    return Err(Error::invalid("refusing to close a slice with changed or foreign claim evidence"));
                }
            }
            if let Some(reason) = existing {
                if reason != request.reason.trim() { return Err(Error::invalid("closure was already requested with another reason; finish that recorded intent")); }
            } else {
                tx.execute("INSERT INTO chat_build_closure(run_id,reason,requested_at) VALUES (?1,?2,?3)", params![control.receipt.run_id,request.reason.trim(),crate::now()])?;
                tx.execute("INSERT INTO event(run_id,at,kind,actor,summary) VALUES (?1,?2,'note','human',?3)", params![control.receipt.run_id,crate::now(),format!("Withdrew this build's approval; close execution and keep all work: {}", request.reason.trim())])?;
            }
            Ok(())
        })
    }

    pub(crate) fn settle_chat_build_close(
        &mut self,
        control: &mut ChatBuildControl,
        request: &ChatBuildClose,
    ) -> Result<()> {
        if !control.ownership.quiescent() {
            return Err(Error::invalid("drain uncertain children before closing"));
        }
        let chat = self.chat(control.receipt.chat_id)?;
        let path = self.planning_path()?;
        self.db_mut().write(|tx| {
            check(tx, control)?;
            let recorded: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_closure WHERE run_id = ?1 AND finished_at IS NULL AND reason = ?2)", params![control.receipt.run_id,request.reason.trim()], |row| row.get(0))?;
            if !recorded { return Err(Error::invalid("no matching unfinished close intent")); }
            let rows = reviewed(tx, control, request)?;
            let mut planner = engine::open(&path, false)?.ok_or_else(|| Error::invalid("the owned planning store is missing"))?;
            let plan = engine::find(&planner, &chat)?.ok_or_else(|| Error::invalid("this chat has no plan"))?;
            let mut issues = Vec::new();
            planner.set_actor(format!("ai-team chat-{} run-{}", chat.id, control.receipt.run_id));
            for row in &rows {
                if !matches_record(&planner, plan.id, chat.id, row)? {
                    issues.push(format!("{}: changed/foreign claim left untouched after close intent", row.slice_key));
                    continue;
                }
                let mut slice = planner.slice_by_id(row.planner_slice_id)?;
                let status = if row.build_status == "verified" && row.commit_sha.is_some() { ai_planner_core::Status::InReview } else { ai_planner_core::Status::Blocked };
                if slice.status != status { slice = planner.set_slice_status(&slice, status, Some(&request.reason))?; }
                if slice.claimed_by.is_some() { planner.release_slice(&slice)?; }
            }
            // Only absence of any acquisition/attempt/publication evidence permits
            // marking a pending reservation released. Every uncertainty stays kept.
            tx.execute("UPDATE chat_build_slice SET lease_state = 'released', build_status = 'stopped', reason = 'Closed before acquisition was attempted', rev = rev + 1
                WHERE run_id = ?1 AND lease_state = 'pending' AND worktree_path IS NULL AND candidate_sha IS NULL AND maker_node_id IS NULL AND verifier_node_id IS NULL", [control.receipt.run_id])?;
            finish(tx, control, &request.reason, &issues)?;
            Ok(())
        })?;
        control.receipt.revision += 1;
        control.receipt.owner = None;
        Ok(())
    }
}

fn check(conn: &Connection, control: &ChatBuildControl) -> Result<()> {
    build::check(conn, control, true)?;
    let undrained: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_child WHERE run_id = ?1 AND state != 'drained')",
        [control.receipt.run_id],
        |row| row.get(0),
    )?;
    if !control.recovering || undrained {
        return Err(Error::invalid(
            "close-and-keep needs a drained human recovery controller",
        ));
    }
    Ok(())
}

fn reviewed(
    conn: &Connection,
    control: &ChatBuildControl,
    request: &ChatBuildClose,
) -> Result<Vec<ChatBuildSlice>> {
    let mut query = conn.prepare(
        "SELECT slice_key FROM chat_build_slice WHERE run_id = ?1 ORDER BY planner_slice_id",
    )?;
    let keys = query
        .query_map([control.receipt.run_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows = keys
        .iter()
        .map(|key| build::read(conn, control.receipt.run_id, key))
        .collect::<Result<Vec<_>>>()?;
    if rows.len() != request.expect_slices.len()
        || rows
            .iter()
            .any(|row| request.expect_slices.get(&row.slice_key) != Some(&row.rev))
    {
        return Err(Error::invalid(
            "the reviewed build slices changed; refresh before closing",
        ));
    }
    Ok(rows)
}

fn matches_record(
    planner: &ai_planner_core::Store,
    plan: i64,
    chat: i64,
    row: &ChatBuildSlice,
) -> Result<bool> {
    let slice = planner.slice_by_id(row.planner_slice_id)?;
    Ok(slice.plan_id == plan
        && slice.key == row.slice_key
        && (slice.claimed_by.is_none() || owned(&slice, row, chat))
        && (slice.branch.is_none() || slice.branch == row.branch))
}

fn finish(
    conn: &Connection,
    control: &ChatBuildControl,
    reason: &str,
    issues: &[String],
) -> Result<()> {
    let at = crate::now();
    let message = format!("Closed by you; retained work remains protected. Nothing was newly verified, returned, merged or discarded. {}", reason.trim());
    conn.execute(
        "UPDATE chat_build_closure SET finished_at = ?2, issues_json = ?3 WHERE run_id = ?1 AND finished_at IS NULL",
        params![control.receipt.run_id, at, serde_json::to_string(issues)?],
    )?;
    conn.execute("UPDATE chat_team_run SET phase = 'finished', reason = ?2, supervisor_pid = NULL, supervisor_identity = NULL, quiescent = 1, rev = rev + 1 WHERE run_id = ?1", params![control.receipt.run_id,message])?;
    conn.execute("UPDATE run SET status = 'cancelled', blocked_reason = ?2, ended_at = ?3, updated_at = ?3, rev = rev + 1 WHERE id = ?1", params![control.receipt.run_id,message,at])?;
    conn.execute("UPDATE chat SET active_node_id = NULL, stop_requested = 0, live_text = '', supervisor_identity = NULL, pi_identity = NULL, updated_at = ?2, rev = rev + 1 WHERE id = ?1", params![control.receipt.chat_id,at])?;
    conn.execute(
        "INSERT INTO event(run_id,at,kind,actor,summary) VALUES (?1,?2,'note','ai-team',?3)",
        params![control.receipt.run_id, at, message],
    )?;
    Ok(())
}
