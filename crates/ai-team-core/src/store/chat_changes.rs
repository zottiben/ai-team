//! Exact chat/draft scope and durable delivery admission. No legacy delivery columns.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::chat_changes::{Delivery, DeliverySnapshot, Finding};
use crate::{Error, Result, Store};

const SELECT: &str = "SELECT id, chat_id, snapshot_json, state, result, rev FROM chat_delivery";

impl Store {
    pub(crate) fn chat_draft_runs(&self, chat: i64) -> Result<Vec<i64>> {
        self.chat(chat)?;
        let mut query = self
            .db()
            .conn()
            .prepare("SELECT run_id FROM chat_team_run WHERE chat_id = ?1 ORDER BY run_id DESC")?;
        let runs = query
            .query_map([chat], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(runs)
    }

    pub(crate) fn chat_draft_findings(
        &self,
        chat: i64,
        target: &crate::chat_changes::DraftTarget,
    ) -> Result<Vec<crate::Event>> {
        check_target(self.db().conn(), chat, target)?;
        let mut query = self.db().conn().prepare("SELECT id FROM event WHERE run_id = ?1 AND actor = 'you' AND json_extract(payload_json, '$.chat_draft_review') IS NOT NULL ORDER BY id")?;
        let ids = query
            .query_map([target.run_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter().map(|id| self.event(id)).collect()
    }

    pub fn chat_deliveries(&self, chat: i64) -> Result<Vec<Delivery>> {
        self.chat(chat)?;
        let mut query = self
            .db()
            .conn()
            .prepare(&format!("{SELECT} WHERE chat_id = ?1 AND state != 'preview' ORDER BY CASE WHEN state IN ('running','inspection') THEN 0 ELSE 1 END, id DESC LIMIT 100"))?;
        let values = query
            .query_map([chat], from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(values)
    }

    pub fn chat_delivery(&self, chat: i64, id: i64) -> Result<Delivery> {
        self.db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id = ?1 AND id = ?2"),
                params![chat, id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no delivery approval in this chat"))
    }

    pub fn review_chat_draft(&mut self, chat: i64, finding: &Finding) -> Result<()> {
        let draft = crate::chat_changes::draft(self, chat, &finding.target)?;
        let body = finding.body.trim();
        if body.is_empty() || body.len() > 32_000 {
            return Err(Error::invalid("a review finding needs 1–32000 bytes"));
        }
        self.db_mut().write(|tx| {
            check_target(tx, chat, &finding.target)?;
            let payload = serde_json::json!({"chat_draft_review": draft.commit_sha, "slice_key": finding.target.slice_key, "body": body});
            event(tx, chat, finding.target.run_id, &format!("Review · {}: {body}", finding.target.slice_key), &payload.to_string())
        })
    }

    pub fn keep_chat_retained_work(
        &mut self,
        chat: i64,
        request: &crate::chat_changes::KeepRetained,
    ) -> Result<()> {
        crate::chat_changes::owned_slice(self, chat, &request.target)?;
        let reason = request.reason.trim();
        if reason.is_empty() || reason.len() > 4000 {
            return Err(Error::invalid(
                "keeping work needs a reason of 1–4000 bytes",
            ));
        }
        self.db_mut().write(|tx| {
            let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice s JOIN chat_team_run t ON t.run_id=s.run_id WHERE t.chat_id=?1 AND s.run_id=?2 AND s.slice_key=?3 AND s.rev=?4 AND s.lease_state NOT IN ('pending','released'))", params![chat, request.target.run_id, request.target.slice_key, request.target.revision], |row| row.get(0))?;
            if !current { return Err(Error::invalid("this retained responsibility changed; refresh before recording its disposition")); }
            event(tx, chat, request.target.run_id, &format!("Keep {} protected: {reason}. No cleanup, lease return or model restart approved.", request.target.slice_key), &serde_json::json!({"chat_retained_keep":request.target.slice_key,"reason":reason}).to_string())
        })
    }

    pub(crate) fn record_chat_delivery_preview(
        &mut self,
        chat: i64,
        snapshot: &DeliverySnapshot,
    ) -> Result<Delivery> {
        let payload = serde_json::to_string(snapshot)?;
        let id = self.db_mut().write(|tx| {
            check_target(tx, chat, &snapshot.target)?;
            check_workspace(tx, &snapshot.workspace_path)?;
            tx.execute("INSERT INTO chat_delivery (chat_id,run_id,slice_key,workspace_path,snapshot_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6)",
                params![chat, snapshot.target.run_id, snapshot.target.slice_key, snapshot.workspace_path, payload, crate::now()])?;
            Ok(tx.last_insert_rowid())
        })?;
        self.chat_delivery(chat, id)
    }

    pub(crate) fn claim_chat_delivery(
        &mut self,
        chat: i64,
        id: i64,
        revision: i64,
    ) -> Result<std::sync::Arc<crate::chat::team::ownership::Ownership>> {
        let delivery = self.chat_delivery(chat, id)?;
        let s = &delivery.snapshot;
        let owner = crate::chat::team::ownership::Ownership::acquire_delivery(
            self.path(),
            s.target.run_id,
            id,
        )?;
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("could not identify delivery supervisor"))?;
        self.db_mut().write(|tx| {
            check_target(tx, chat, &s.target)?;
            check_workspace(tx, &s.workspace_path)?;
            let project: i64 = tx.query_row("SELECT project_id FROM chat WHERE id = ?1", [chat], |row| row.get(0))?;
            super::chat_teams::kept::check_unlocated(tx, project, Some(&s.workspace_path), None)?;
            let idle: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat WHERE id = ?1 AND active_node_id IS NULL AND archived = 0 AND workspace_path = ?2)", params![chat, s.workspace_path], |row| row.get(0))?;
            if !idle { return Err(Error::invalid("this chat is no longer idle at the approved checkout")); }
            let mut query = tx.prepare("SELECT workspace_path FROM chat WHERE active_node_id IS NOT NULL
                UNION SELECT workspace_path FROM run WHERE status IN ('queued','planning','running') AND supervisor_pid IS NOT NULL AND workspace_path IS NOT NULL
                UNION SELECT worktree_path FROM node_run WHERE status NOT IN ('done','failed','cancelled') AND worktree_path IS NOT NULL
                UNION SELECT worktree_path FROM chat_build_slice WHERE lease_state != 'released' AND worktree_path IS NOT NULL")?;
            let paths = query.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            if paths.iter().any(|path| crate::same_worktree(path, &s.workspace_path)) { return Err(Error::invalid("another execution or retained lease owns the delivery checkout")); }
            drop(query);
            if tx.execute("UPDATE chat_delivery SET state = 'running', supervisor_pid = ?5, supervisor_identity = ?6, child_journal = 1, rev = rev + 1, updated_at = ?4 WHERE id = ?1 AND chat_id = ?2 AND rev = ?3 AND state = 'preview'", params![id, chat, revision, crate::now(), pid, identity])? != 1 {
                return Err(Error::invalid("this delivery approval was used or changed; refresh before retrying"));
            }
            tx.execute("UPDATE chat_team_run SET child_epoch = child_epoch + 1 WHERE run_id = ?1", [s.target.run_id])?;
            event(tx, chat, s.target.run_id, &format!("Approved {:?} for {} at {}", s.action, s.target.slice_key, s.commit_sha), &serde_json::json!({"chat_delivery":id, "state":"running"}).to_string())
        })?;
        owner.bind(self)?;
        Ok(owner)
    }

    pub(crate) fn attempt_chat_delivery(&mut self, chat: i64, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_delivery SET attempted = 1 WHERE chat_id = ?1 AND id = ?2 AND state = 'running' AND supervisor_pid = ?3", params![chat, id, i64::from(std::process::id())])? != 1 { return Err(Error::invalid("delivery is no longer owned")); }
            Ok(())
        })
    }

    pub(crate) fn reclaim_chat_delivery(
        &mut self,
        chat: i64,
        id: i64,
        revision: i64,
    ) -> Result<(
        std::sync::Arc<crate::chat::team::ownership::Ownership>,
        bool,
    )> {
        let delivery = self.chat_delivery(chat, id)?;
        let run = delivery.snapshot.target.run_id;
        let owner =
            crate::chat::team::ownership::Ownership::acquire_delivery(self.path(), run, id)?;
        let pid = i64::from(std::process::id());
        let identity = crate::chat::process_identity(pid)
            .ok_or_else(|| Error::invalid("could not identify delivery supervisor"))?;
        let attempted = self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_delivery SET state = 'running', supervisor_pid = ?4, supervisor_identity = ?5, rev = rev + 1, updated_at = ?6 WHERE id = ?1 AND chat_id = ?2 AND rev = ?3 AND state IN ('running','inspection') AND child_journal = 1", params![id, chat, revision, pid, identity, crate::now()])? != 1 { return Err(Error::invalid("delivery changed or has no process journal; refresh or inspect manually")); }
            tx.execute("UPDATE chat_team_run SET child_epoch = child_epoch + 1 WHERE run_id = ?1", [run])?;
            Ok(tx.query_row("SELECT attempted FROM chat_delivery WHERE id = ?1", [id], |row| row.get(0))?)
        })?;
        owner.bind(self)?;
        Ok((owner, attempted))
    }

    pub(crate) fn settle_chat_delivery(
        &mut self,
        chat: i64,
        id: i64,
        state: &str,
        result: &str,
    ) -> Result<Delivery> {
        if !matches!(state, "done" | "refused" | "inspection" | "acknowledged") {
            return Err(Error::invalid("invalid delivery outcome"));
        }
        let delivery = self.chat_delivery(chat, id)?;
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat_delivery SET state = ?3, result = ?4, supervisor_pid = NULL, supervisor_identity = NULL, rev = rev + 1, updated_at = ?5 WHERE id = ?1 AND chat_id = ?2 AND state = 'running'", params![id, chat, state, result, crate::now()])? != 1 {
                return Err(Error::invalid("this delivery is no longer owned by its command"));
            }
            event(tx, chat, delivery.snapshot.target.run_id, result, &serde_json::json!({"chat_delivery":id,"state":state}).to_string())
        })?;
        self.chat_delivery(chat, id)
    }
}

pub(super) fn check_child_owner(
    conn: &Connection,
    owner: &crate::chat::team::ownership::Ownership,
    delivery: i64,
) -> Result<()> {
    let current: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_delivery d JOIN chat_team_run t ON t.run_id = d.run_id WHERE d.id = ?1 AND d.run_id = ?2 AND t.child_epoch = ?3 AND d.state = 'running' AND d.child_journal = 1 AND d.supervisor_pid = ?4)", params![delivery, owner.run, owner.epoch(), i64::from(std::process::id())], |row| row.get(0))?;
    if !current || owner.epoch() == 0 {
        return Err(Error::invalid(
            "this delivery no longer owns its command journal",
        ));
    }
    Ok(())
}

fn check_target(
    conn: &Connection,
    chat: i64,
    target: &crate::chat_changes::DraftTarget,
) -> Result<()> {
    let found: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice s JOIN chat_team_run t ON t.run_id = s.run_id WHERE t.chat_id = ?1 AND s.run_id = ?2 AND s.slice_key = ?3 AND s.rev = ?4 AND s.build_status = 'verified' AND s.commit_sha IS NOT NULL)",
        params![chat, target.run_id, target.slice_key, target.revision], |row| row.get(0))?;
    if !found {
        return Err(Error::invalid(
            "this verified draft changed or belongs to another chat",
        ));
    }
    Ok(())
}

/// Shared by chat, leased maker and legacy admission. A process dying is not proof that
/// its external command did nothing. Unknown outcomes continue to protect the checkout.
pub(super) fn check_workspace(conn: &Connection, workspace: &str) -> Result<()> {
    check_workspace_except_toolbox(conn, workspace, None)
}
pub(super) fn check_workspace_except_toolbox(
    conn: &Connection,
    workspace: &str,
    approval: Option<i64>,
) -> Result<()> {
    let mut query = conn.prepare(
        "SELECT workspace_path FROM chat_delivery WHERE state IN ('running','inspection')
         UNION SELECT json_extract(snapshot_json,'$.authority.target') FROM toolbox_operation
         WHERE kind='converge' AND state='applying' AND id != COALESCE(?1,-1)",
    )?;
    let paths = query
        .query_map([approval], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if paths
        .iter()
        .any(|path| crate::same_worktree(path, workspace))
    {
        return Err(Error::invalid(
            "a chat delivery or toolbox convergence is running or needs inspection in this checkout",
        ));
    }
    Ok(())
}

fn event(conn: &Connection, chat: i64, run: i64, summary: &str, payload: &str) -> Result<()> {
    conn.execute("INSERT INTO event (run_id,at,kind,actor,summary,payload_json) VALUES (?1,?2,'note','you',?3,?4)", params![run, crate::now(), summary, payload])?;
    conn.execute(
        "UPDATE chat SET rev = rev + 1, updated_at = ?2 WHERE id = ?1",
        params![chat, crate::now()],
    )?;
    Ok(())
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Delivery> {
    let json: String = row.get(2)?;
    let snapshot = serde_json::from_str(&json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(Delivery {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        snapshot,
        state: row.get(3)?,
        result: row.get(4)?,
        rev: row.get(5)?,
    })
}
