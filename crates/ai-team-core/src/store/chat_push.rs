//! Rows for explicit human push authority. Nothing here decides what a message meant.
//!
//! A grant is only ever as good as the chat it was minted against: the same checkout, the
//! same workspace generation, the same mode, the same supervising process, and - for a
//! solo turn - the same active turn. Those facts are rechecked on every read, so a new
//! instruction, a Stop, an archive, a workspace handoff, a mode change or a restart cannot
//! revive one. A grant that no longer holds is expired, never silently reused.

mod admission;
pub(super) use admission::{check_operation, mint_solo};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::chat_push::{NewPushGrant, PushGrant};
use crate::{Error, Result, Store};

const SELECT: &str = "SELECT id,chat_id,request_id,node_id,mode,workspace_path,workspace_epoch,
    branch,origin_url,head_sha,allow_commit,supervisor_pid,supervisor_identity,commit_sha,
    operation_id,state,result,draft_json,new_branch_json,pinned_branch FROM chat_push_grant";

fn from_row(row: &Row<'_>) -> rusqlite::Result<PushGrant> {
    Ok(PushGrant {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        request_id: row.get(2)?,
        node_id: row.get(3)?,
        draft: row
            .get::<_, Option<String>>(17)?
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    17,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        mode: row.get(4)?,
        workspace_path: row.get(5)?,
        workspace_epoch: row.get(6)?,
        branch: row.get(7)?,
        new_branch: row
            .get::<_, Option<String>>(18)?
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    18,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        pinned_branch: row.get(19)?,
        origin_url: row.get(8)?,
        head_sha: row.get(9)?,
        allow_commit: row.get(10)?,
        supervisor_pid: row.get(11)?,
        supervisor_identity: row.get(12)?,
        commit_sha: row.get(13)?,
        operation_id: row.get(14)?,
        state: row.get(15)?,
        result: row.get(16)?,
    })
}

/// What a grant has to still be true about, expressed where the rows are. The workspace
/// generation is recomputed the way `chat.workspace_epoch` is, rather than trusted.
const STILL_HOLDS: &str = "state IN ('pending','armed') AND EXISTS(
    SELECT 1 FROM chat c WHERE c.id=chat_push_grant.chat_id AND c.archived=0
        AND c.stop_requested=0 AND c.mode=chat_push_grant.mode
        AND c.workspace_path=chat_push_grant.workspace_path
        AND ((chat_push_grant.node_id IS NULL AND c.active_node_id IS NULL)
          OR (chat_push_grant.node_id IS NOT NULL
            AND chat_push_grant.node_id=(SELECT MAX(node_id) FROM chat_turn WHERE chat_id=c.id)
            AND (c.active_node_id=chat_push_grant.node_id OR (c.active_node_id IS NULL
              AND EXISTS(SELECT 1 FROM node_run n WHERE n.id=chat_push_grant.node_id AND n.status='done')))))
        AND COALESCE((SELECT MAX(w.id) FROM chat_workspace_request w
            WHERE w.chat_id=c.id AND w.state='applied'),0)=chat_push_grant.workspace_epoch)";

impl Store {
    pub(crate) fn chat_push_creates_branch(&self, chat: i64, operation: i64) -> Result<bool> {
        admission::check_operation(self.db().conn(), chat, operation)
    }

    pub fn chat_push_grant(&self, chat: i64, id: i64) -> Result<PushGrant> {
        self.db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND id=?2"),
                params![chat, id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| Error::invalid("no push authority in this chat"))
    }

    /// The grant one exact human request minted, whatever became of it.
    pub fn chat_push_grant_for_request(
        &self,
        chat: i64,
        request_id: &str,
    ) -> Result<Option<PushGrant>> {
        Ok(self
            .db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND request_id=?2"),
                params![chat, request_id],
                from_row,
            )
            .optional()?)
    }

    pub fn replay_chat_push(
        &self,
        chat: i64,
        request: &str,
        message: &str,
    ) -> Result<Option<crate::ChatSubmission>> {
        let Some(grant) = self.chat_push_grant_for_request(chat, request)? else {
            return Ok(None);
        };
        let recorded: String = self.db().conn().query_row(
            "SELECT message FROM chat_push_grant WHERE id=?1",
            [grant.id],
            |r| r.get(0),
        )?;
        if recorded != message.trim() {
            return Err(Error::invalid(
                "that push request id belongs to another message",
            ));
        }
        let (run_id, node_id) = if let Some(node) = grant.node_id {
            (self.node_run(node)?.run_id, node)
        } else {
            let target = grant
                .draft
                .ok_or_else(|| Error::invalid("push evidence has no exact draft"))?;
            let team = self
                .chat_team_run(target.run_id)?
                .ok_or_else(|| Error::invalid("push evidence has no exact execution"))?;
            (team.run_id, team.control_node_id)
        };
        Ok(Some(crate::ChatSubmission {
            run_id,
            node_id,
            started: false,
        }))
    }

    /// This chat's live authority, if it still holds. A dead supervisor's grant does not.
    pub fn live_chat_push_grant(&self, chat: i64) -> Result<Option<PushGrant>> {
        let found = self
            .db()
            .conn()
            .query_row(
                &format!("{SELECT} WHERE chat_id=?1 AND {STILL_HOLDS}"),
                [chat],
                from_row,
            )
            .optional()?;
        Ok(found.filter(PushGrant::supervised))
    }

    /// Whether this solo turn may still pin a commit for publication.
    pub fn chat_push_grant_for_node(&self, chat: i64, node: i64) -> Result<Option<PushGrant>> {
        Ok(self
            .live_chat_push_grant(chat)?
            .filter(|grant| grant.node_id == Some(node)))
    }

    /// Expire every live grant that no longer holds, for one chat or for all of them.
    ///
    /// Called before minting and once per process start. A grant whose supervisor is gone
    /// is the restart case: its process can no longer be the one the person was talking to.
    pub fn expire_stale_chat_push_grants(&mut self, chat: Option<i64>) -> Result<usize> {
        let mut query = self
            .db()
            .conn()
            .prepare(&format!("{SELECT} WHERE state IN ('pending','armed')"))?;
        let live: Vec<PushGrant> = query
            .query_map([], from_row)?
            .collect::<rusqlite::Result<_>>()?;
        drop(query);
        let mut stale = Vec::new();
        for grant in live
            .iter()
            .filter(|grant| chat.is_none_or(|id| grant.chat_id == id))
        {
            let reason = if grant.supervised() {
                let holds: bool = self.db().conn().query_row(&format!("SELECT EXISTS(SELECT 1 FROM chat_push_grant WHERE id=?1 AND {STILL_HOLDS})"), [grant.id], |row| row.get(0))?;
                (!holds).then_some("The chat changed. Permission expired; inspect any recorded checkout operation for the publication outcome.")
            } else {
                Some("The approving process is gone. Permission expired; inspect any recorded checkout operation for the publication outcome.")
            };
            if let Some(reason) = reason {
                stale.push((grant.id, reason));
            }
        }
        let count = stale.len();
        for (id, reason) in stale {
            self.settle_chat_push_grant(id, "expired", reason)?;
        }
        Ok(count)
    }

    /// Record one authority. Replaying the same human request returns what it minted.
    pub(crate) fn mint_chat_push_grant(&mut self, new: &NewPushGrant) -> Result<PushGrant> {
        let id = self.db_mut().write(|tx| admission::mint(tx, new))?;
        self.chat_push_grant(new.chat_id, id)
    }

    /// Pin the commit the authorised turn made. Set once; a second request must match it.
    pub(crate) fn arm_chat_push_grant(
        &mut self,
        chat: i64,
        node: i64,
        branch: &str,
        commit: &str,
    ) -> Result<PushGrant> {
        let grant = self.chat_push_grant_for_node(chat, node)?.ok_or_else(|| {
            Error::invalid("this turn has no push authority from the person in this chat")
        })?;
        if let Some(pinned) = &grant.commit_sha {
            if pinned != commit || grant.publication_branch() != branch {
                return Err(Error::invalid(
                    "a different commit is already pinned for this push; it will not be retargeted",
                ));
            }
            return Ok(grant);
        }
        let id = grant.id;
        self.db_mut().write(|tx| {
            if tx.execute(
                &format!(
                    "UPDATE chat_push_grant SET commit_sha=?2,pinned_branch=?6,state='armed',updated_at=?3
                 WHERE id=?1 AND state='pending' AND commit_sha IS NULL AND {STILL_HOLDS}
                 AND EXISTS(SELECT 1 FROM chat WHERE id=?4 AND active_node_id=?5)"
                ),
                params![id, commit, crate::now(), chat, node, branch],
            )? != 1
            {
                return Err(Error::invalid("this push authority is no longer pending"));
            }
            tick(tx, chat)
        })?;
        self.chat_push_grant(chat, id)
    }

    /// Bind the journalled operation that will publish it, before it runs. Set once, so a
    /// replay inspects that receipt rather than starting a second push.
    pub(crate) fn bind_chat_push_operation(&mut self, id: i64, operation: i64) -> Result<()> {
        self.db_mut().write(|tx| {
            if tx.execute(
                &format!(
                    "UPDATE chat_push_grant SET operation_id=?2,updated_at=?3
                 WHERE id=?1 AND state='armed' AND operation_id IS NULL AND {STILL_HOLDS}"
                ),
                params![id, operation, crate::now()],
            )? != 1
            {
                return Err(Error::invalid("this push already has a recorded attempt"));
            }
            Ok(())
        })
    }

    pub(crate) fn settle_chat_push_grant(
        &mut self,
        id: i64,
        state: &str,
        result: &str,
    ) -> Result<PushGrant> {
        if !matches!(state, "spent" | "expired") {
            return Err(Error::invalid("invalid push outcome"));
        }
        let chat: i64 = self.db_mut().write(|tx| {
            let chat: i64 = tx.query_row(
                "SELECT chat_id FROM chat_push_grant WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            if tx.execute(
                "UPDATE chat_push_grant SET state=?2,result=?3,updated_at=?4
                 WHERE id=?1 AND state IN ('pending','armed')",
                params![id, state, result, crate::now()],
            )? != 1
            {
                return Err(Error::invalid("this push authority is already settled"));
            }
            tick(tx, chat)?;
            Ok(chat)
        })?;
        self.chat_push_grant(chat, id)
    }

    /// Say in the conversation that a message which mentioned publishing issued nothing.
    pub fn note_chat_push_refusal(&mut self, chat: i64, node: i64, reason: &str) -> Result<()> {
        self.record_chat_push_note(chat, Some(node), "No push authorised", reason)
    }

    /// Put what the host actually did into the conversation, on the turn it belongs to.
    pub(crate) fn record_chat_push_note(
        &mut self,
        chat: i64,
        node: Option<i64>,
        summary: &str,
        body: &str,
    ) -> Result<()> {
        let payload = serde_json::to_string(&serde_json::json!({"body": body}))?;
        self.db_mut().write(|tx| {
            let target: Option<(i64, i64)> = match node {
                Some(node) => tx.query_row("SELECT run_id,node_id FROM chat_turn WHERE chat_id=?1 AND node_id=?2", params![chat, node], |row| Ok((row.get(0)?, row.get(1)?))).optional()?,
                None => tx.query_row("SELECT run_id,node_id FROM chat_turn WHERE chat_id=?1 ORDER BY node_id DESC LIMIT 1", [chat], |row| Ok((row.get(0)?, row.get(1)?))).optional()?,
            };
            let Some((run, node)) = target else { return Ok(()) };
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, 'note', 'ai-team', ?4, ?5)",
                params![run, node, crate::now(), summary, payload],
            )?;
            tick(tx, chat)
        })
    }
}

fn check_epoch(conn: &Connection, chat: i64, expected: i64) -> Result<()> {
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(id),0) FROM chat_workspace_request WHERE chat_id=?1 AND state='applied'",
        [chat],
        |row| row.get(0),
    )?;
    if current != expected {
        return Err(Error::invalid(
            "this chat's checkout changed since this message was composed; send it again",
        ));
    }
    Ok(())
}

fn tick(conn: &Connection, chat: i64) -> Result<()> {
    conn.execute(
        "UPDATE chat SET rev=rev+1,updated_at=?2 WHERE id=?1",
        params![chat, crate::now()],
    )?;
    Ok(())
}
