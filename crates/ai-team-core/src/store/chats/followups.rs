//! Boundary steering for Pi's print transport: stop/drain, then a new supervised turn.
//! Queue admission is not delivery. Only an exact user-prompt echo confirms delivery.
use rusqlite::{params, OptionalExtension, Row};
use sha2::{Digest, Sha256};

use crate::chat::{ChatFollowup, ChatSubmission, FollowupKind};
use crate::{Error, ModelRegistry, NodeStatus, Result, Store};

const SELECT: &str = "SELECT id,chat_id,after_node_id,body,kind,state,node_id,created_at,delivered_at FROM chat_followup";

impl Store {
    pub fn chat_followups(&self, chat: i64) -> Result<Vec<ChatFollowup>> {
        self.chat(chat)?;
        let mut query = self.db().conn().prepare(&format!(
            "{SELECT} WHERE chat_id=?1 ORDER BY id DESC LIMIT 10"
        ))?;
        let mut rows: Vec<_> = query
            .query_map([chat], from_row)?
            .collect::<rusqlite::Result<_>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn chat_followup(&self, chat: i64, id: i64) -> Result<ChatFollowup> {
        self.db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND id=?2"),
                params![chat, id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no queued instruction in this chat"))
    }

    pub fn queue_chat_followup(
        &mut self,
        chat: i64,
        after: i64,
        body: &str,
        request: &str,
        kind: FollowupKind,
    ) -> Result<ChatFollowup> {
        let body = body.trim();
        super::validate_message(body, request)?;
        let kind = match kind {
            FollowupKind::FollowUp => "follow_up",
            FollowupKind::Steer => "steer",
        };
        let id = self.db_mut().write(|tx| {
            if let Some((id, prior, text, intent)) = tx.query_row("SELECT id,after_node_id,body,kind FROM chat_followup WHERE chat_id=?1 AND request_id=?2", params![chat,request], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()? {
                if prior != after || text != body || intent != kind { return Err(Error::invalid("that request id belongs to another queued instruction")); }
                return Ok(id);
            }
            super::super::chat_workspaces::pending(tx, chat)?;
            let allowed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat c JOIN chat_turn t ON t.chat_id=c.id JOIN node_run n ON n.id=t.node_id JOIN project p ON p.id=c.project_id WHERE c.id=?1 AND c.active_node_id=?2 AND t.node_id=?2 AND c.archived=0 AND p.status!='archived' AND c.mode='single' AND c.stop_requested=0 AND n.status='running' AND NOT EXISTS(SELECT 1 FROM chat_team_run tr WHERE tr.run_id=t.run_id))", params![chat,after], |r| r.get(0))?;
            if !allowed { return Err(Error::invalid("that solo turn is no longer accepting instructions; inspect its receipts, or clear this draft before composing for another turn")); }
            let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state='queued')", [chat], |r| r.get(0))?;
            if pending { return Err(Error::invalid("this chat already has a queued instruction; cancel it before replacing it")); }
            tx.execute("INSERT INTO chat_followup(chat_id,after_node_id,request_id,body,kind,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![chat,after,request,body,kind,crate::now()])?;
            let id = tx.last_insert_rowid();
            tx.execute("UPDATE chat SET stop_requested=CASE WHEN ?2='steer' THEN 1 ELSE stop_requested END, rev=rev+1, updated_at=?3 WHERE id=?1", params![chat,kind,crate::now()])?;
            note(tx, after, if kind == "steer" { "Steering queued; stopping and draining this exact turn before starting a follow-up. Not delivered yet." } else { "Follow-up queued for this turn's completion. Not delivered yet." })?;
            Ok(id)
        })?;
        self.chat_followup(chat, id)
    }

    pub fn cancel_chat_followup(&mut self, chat: i64, id: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            let after: Option<i64> = tx.query_row("SELECT after_node_id FROM chat_followup WHERE chat_id=?1 AND id=?2 AND state='queued'", params![chat,id], |r| r.get(0)).optional()?;
            let after = after.ok_or_else(|| Error::invalid("this instruction was already claimed or cancelled; refresh before acting"))?;
            tx.execute("UPDATE chat_followup SET state='cancelled' WHERE id=?1", [id])?;
            tx.execute("UPDATE chat SET rev=rev+1,updated_at=?2 WHERE id=?1", params![chat,crate::now()])?;
            note(tx,after,"Queued instruction cancelled. An already requested stop is not undone.")
        })
    }

    /// Human recovery of an unclaimed queue. A claimed/uncertain delivery is never retried.
    pub fn send_chat_followup(
        &mut self,
        chat: i64,
        id: i64,
        registry: &ModelRegistry,
    ) -> Result<ChatSubmission> {
        let queued = self.chat_followup(chat, id)?;
        if let Some(node) = queued.node_id {
            return Ok(ChatSubmission {
                run_id: self.node_run(node)?.run_id,
                node_id: node,
                started: false,
            });
        }
        if queued.state != "queued" {
            return Err(Error::invalid("this queued instruction was cancelled"));
        }
        self.begin_chat_turn_inner(
            chat,
            &queued.body,
            &format!("followup/{id}"),
            registry,
            super::TurnOptions {
                followup: Some(id),
                ..super::TurnOptions::default()
            },
        )
    }

    /// Called only by the worker that just drained this predecessor. Startup/read paths
    /// do not call this: after a crash an unclaimed queue awaits explicit human action.
    pub fn advance_chat_followup(
        &mut self,
        chat: i64,
        after: i64,
        registry: &ModelRegistry,
    ) -> Result<Option<ChatSubmission>> {
        let queued = self
            .db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND after_node_id=?2 AND state='queued'"),
                params![chat, after],
                from_row,
            )
            .optional()?;
        let Some(queued) = queued else {
            return Ok(None);
        };
        let status = self.node_run(after)?.status;
        if status != NodeStatus::Done
            && !(status == NodeStatus::Cancelled && queued.kind == FollowupKind::Steer)
        {
            return Ok(None);
        }
        self.send_chat_followup(chat, queued.id, registry).map(Some)
    }

    pub fn record_chat_followup_problem(
        &mut self,
        chat: i64,
        after: i64,
        reason: &str,
    ) -> Result<()> {
        self.db_mut().write(|tx| {
            let held: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND after_node_id=?2 AND state='queued')",params![chat,after],|r|r.get(0))?;
            if held {
                note(tx,after,&format!("Follow-up held: {reason}. Send it explicitly or cancel it; no automatic retry."))?;
                tx.execute("UPDATE chat SET rev=rev+1,updated_at=?2 WHERE id=?1",params![chat,crate::now()])?;
            }
            Ok(())
        })
    }

    pub(crate) fn prepare_chat_followup_prompt(&mut self, node: i64, prompt: &str) -> Result<()> {
        let hash = format!("{:x}", Sha256::digest(prompt.as_bytes()));
        self.db_mut().write(|tx| {
            tx.execute(
                "UPDATE chat_followup SET prompt_sha=?2 WHERE node_id=?1 AND state='starting'",
                params![node, hash],
            )?;
            Ok(())
        })
    }
}

pub(super) fn cancel_on_stop(conn: &rusqlite::Connection, chat: i64, node: i64) -> Result<()> {
    if conn.execute("UPDATE chat_followup SET state='cancelled' WHERE chat_id=?1 AND after_node_id=?2 AND state='queued'", params![chat,node])? != 0 {
        note(conn,node,"Queued instruction cancelled by Stop turn; it will not be sent.")?;
    }
    Ok(())
}

pub(super) fn check_admission(
    conn: &rusqlite::Connection,
    chat: i64,
    message: &str,
    followup: Option<i64>,
) -> Result<()> {
    if let Some(followup) = followup {
        let exact: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_followup f JOIN chat c ON c.id=f.chat_id WHERE f.id=?1 AND f.chat_id=?2 AND f.state='queued' AND f.body=?3 AND c.mode='single' AND f.after_node_id=(SELECT node_id FROM chat_turn WHERE chat_id=?2 ORDER BY run_id DESC LIMIT 1))", params![followup,chat,message], |r| r.get(0))?;
        if !exact {
            return Err(Error::invalid(
                "this queued instruction or its predecessor changed; refresh before sending",
            ));
        }
    } else {
        let queued: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state='queued')",
            [chat],
            |r| r.get(0),
        )?;
        if queued {
            return Err(Error::invalid(
                "send or cancel this chat's queued instruction first",
            ));
        }
    }
    Ok(())
}

pub(crate) fn acknowledge(
    conn: &rusqlite::Connection,
    node: i64,
    event: &crate::PiEvent,
) -> Result<()> {
    if !matches!(event.kind.as_str(), "message_start" | "message_end")
        || event.data.pointer("/message/role").and_then(|v| v.as_str()) != Some("user")
    {
        return Ok(());
    }
    let Some(content) = event
        .data
        .pointer("/message/content")
        .and_then(|v| v.as_array())
    else {
        return Ok(());
    };
    let text: String = content
        .iter()
        .filter(|v| v["type"] == "text")
        .filter_map(|v| v["text"].as_str())
        .collect();
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    if conn.execute("UPDATE chat_followup SET state='delivered',delivered_at=?3 WHERE node_id=?1 AND state='starting' AND prompt_sha=?2",params![node,hash,crate::now()])? == 1 {
        conn.execute("UPDATE chat SET rev=rev+1 WHERE id=(SELECT chat_id FROM chat_followup WHERE node_id=?1)", [node])?;
        note(conn,node,"Queued instruction delivered to Pi (exact prompt acknowledged). This is not evidence that the requested work is complete.")?;
    }
    Ok(())
}

fn note(conn: &rusqlite::Connection, node: i64, summary: &str) -> Result<()> {
    conn.execute("INSERT INTO event(run_id,node_run_id,at,kind,actor,summary) SELECT run_id,id,?2,'note','ai-team',?3 FROM node_run WHERE id=?1",params![node,crate::now(),summary])?;
    Ok(())
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<ChatFollowup> {
    Ok(ChatFollowup {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        after_node_id: row.get(2)?,
        body: row.get(3)?,
        kind: if row.get::<_, String>(4)? == "steer" {
            FollowupKind::Steer
        } else {
            FollowupKind::FollowUp
        },
        state: row.get(5)?,
        node_id: row.get(6)?,
        created_at: row.get(7)?,
        delivered_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests;
