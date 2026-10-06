//! Checkout changes serialize with prompt admission and keep the previous execution intact.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::chat_workspaces::WorkspaceRequest;
use crate::planning::{PlanAccess, PlanActor};
use crate::{Chat, Error, Result, Store};

const SELECT: &str =
    "SELECT id, chat_id, after_node_id, from_path, to_path, state FROM chat_workspace_request";

fn row(row: &Row<'_>) -> rusqlite::Result<WorkspaceRequest> {
    Ok(WorkspaceRequest {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        after_node_id: row.get(2)?,
        from_path: row.get(3)?,
        to_path: row.get(4)?,
        state: row.get(5)?,
    })
}

impl Store {
    pub fn chat_workspace_requests(&self, chat: i64) -> Result<Vec<WorkspaceRequest>> {
        self.chat(chat)?;
        let mut query = self.db().conn().prepare(&format!(
            "{SELECT} WHERE chat_id=?1 ORDER BY id DESC LIMIT 20"
        ))?;
        let found = query
            .query_map([chat], row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(found)
    }

    pub fn chat_workspace_request(&self, chat: i64, request: i64) -> Result<WorkspaceRequest> {
        self.db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND id=?2"),
                params![chat, request],
                row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no worktree request in this chat"))
    }

    /// The service resolves Git membership before calling this; no process moves here.
    pub fn request_chat_workspace(
        &mut self,
        chat: i64,
        actor: PlanActor,
        target: &Path,
    ) -> Result<WorkspaceRequest> {
        let target = target.canonicalize()?.to_string_lossy().into_owned();
        let id = self.db_mut().write(|tx| {
            let current = tx.query_row(&format!("{} WHERE id=?1", super::chats::SELECT), [chat], super::chats::from_row)?;
            if current.archived { return Err(Error::invalid("restore this chat before choosing a worktree")); }
            if let PlanActor::Agent(node) = actor {
                let (access, _) = super::planning::scope(tx, chat, actor, false)?;
                if current.active_node_id != Some(node) || !matches!(access, PlanAccess::Planner | PlanAccess::Coordinator) {
                    return Err(Error::invalid("only this chat's coordinating turn may propose a checkout"));
                }
            }
            if crate::same_worktree(&current.workspace_path, &target) {
                return Err(Error::invalid("this chat already uses that checkout"));
            }
            let queued: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state IN ('queued','starting'))", [chat], |row| row.get(0))?;
            if queued { return Err(Error::invalid("resolve the queued instruction before changing its checkout")); }
            let pending = tx.query_row(&format!("{SELECT} WHERE chat_id=?1 AND state='pending'"), [chat], row).optional()?;
            if let Some(pending) = pending {
                if pending.to_path == target { return Ok(pending.id); }
                return Err(Error::invalid("approve or cancel the existing worktree request first"));
            }
            tx.execute("INSERT INTO chat_workspace_request(chat_id,after_node_id,from_path,to_path,created_at) VALUES (?1,?2,?3,?4,?5)",
                params![chat, latest(tx,chat)?, current.workspace_path, target, crate::now()])?;
            let id = tx.last_insert_rowid();
            tick(tx, chat)?;
            Ok(id)
        })?;
        self.chat_workspace_request(chat, id)
    }

    /// Human only. Revalidation of Git/awt belongs immediately before this write.
    pub fn apply_chat_workspace(
        &mut self,
        chat: i64,
        request: i64,
        expect_revision: i64,
        resolved: &Path,
    ) -> Result<Chat> {
        let target = resolved.canonicalize()?.to_string_lossy().into_owned();
        self.db_mut().write(|tx| {
            let current = tx.query_row(&format!("{} WHERE id=?1", super::chats::SELECT), [chat], super::chats::from_row)?;
            let proposed = tx.query_row(&format!("{SELECT} WHERE chat_id=?1 AND id=?2"), params![chat,request], row)?;
            if proposed.state == "applied" && current.workspace_path == proposed.to_path { return Ok(()); }
            if current.rev != expect_revision || current.archived || current.active_node_id.is_some() {
                return Err(Error::invalid("the chat changed or is still working; wait for it to settle and refresh"));
            }
            if proposed.state != "pending" || proposed.from_path != current.workspace_path || proposed.to_path != target || proposed.after_node_id != latest(tx,chat)? {
                return Err(Error::invalid("this worktree request is stale; cancel it and choose the checkout again"));
            }
            let held: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state IN ('queued','starting')) OR EXISTS(SELECT 1 FROM chat_team_run t JOIN chat_build_slice s ON s.run_id=t.run_id WHERE t.chat_id=?1 AND s.lease_state NOT IN ('pending','released'))", [chat], |row| row.get(0))?;
            if held { return Err(Error::invalid("this chat has a queued instruction or retained team work; resolve it before switching checkouts")); }
            super::chat_changes::check_workspace(tx, &current.workspace_path)?;
            super::chats::check_chat_admission(tx, &current, &target, true)?;
            tx.execute("UPDATE chat SET workspace_path=?2, stop_requested=0, rev=rev+1, updated_at=?3 WHERE id=?1", params![chat,target,crate::now()])?;
            tx.execute("UPDATE chat_workspace_request SET state='applied',settled_at=?2 WHERE id=?1", params![request,crate::now()])?;
            if let Some(node) = proposed.after_node_id {
                tx.execute("INSERT INTO event(run_id,node_run_id,at,kind,actor,summary,payload_json) SELECT run_id,id,?2,'note','you','Chat checkout changed',?3 FROM node_run WHERE id=?1", params![node,crate::now(),serde_json::json!({"chat_workspace_change":request,"from":proposed.from_path,"to":target}).to_string()])?;
            }
            Ok(())
        })?;
        self.chat(chat)
    }

    pub fn cancel_chat_workspace(&mut self, chat: i64, request: i64) -> Result<()> {
        self.chat_workspace_request(chat, request)?;
        self.db_mut().write(|tx| {
            tx.execute("UPDATE chat_workspace_request SET state='cancelled',settled_at=?3 WHERE chat_id=?1 AND id=?2 AND state='pending'", params![chat,request,crate::now()])?;
            tick(tx,chat)
        })
    }
}

fn latest(conn: &Connection, chat: i64) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT node_id FROM chat_turn WHERE chat_id=?1 ORDER BY run_id DESC LIMIT 1",
            [chat],
            |row| row.get(0),
        )
        .optional()?)
}
fn tick(conn: &Connection, chat: i64) -> Result<()> {
    conn.execute(
        "UPDATE chat SET rev=rev+1,updated_at=?2 WHERE id=?1",
        params![chat, crate::now()],
    )?;
    Ok(())
}

pub(super) fn check_epoch(conn: &Connection, chat: i64, expected: Option<i64>) -> Result<()> {
    if let Some(expected) = expected {
        let current: i64 = conn.query_row("SELECT COALESCE(MAX(id),0) FROM chat_workspace_request WHERE chat_id=?1 AND state='applied'", [chat], |row| row.get(0))?;
        if current != expected {
            return Err(Error::invalid("this chat's checkout changed since this message was composed; refresh before sending"));
        }
    }
    Ok(())
}

pub(super) fn pending(conn: &Connection, chat: i64) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_workspace_request WHERE chat_id=?1 AND state='pending')",
        [chat],
        |row| row.get(0),
    )?;
    if exists {
        return Err(Error::invalid(
            "approve or cancel this chat's worktree request before sending more work",
        ));
    }
    Ok(())
}
