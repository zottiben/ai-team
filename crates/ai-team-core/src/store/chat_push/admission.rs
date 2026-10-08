use super::{check_epoch, from_row, tick, SELECT, STILL_HOLDS};
use crate::chat_push::NewPushGrant;
use crate::chat_push::{classify, Intent};
use crate::{ChatMode, Error, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub(in crate::store) fn mint_solo(
    conn: &Connection,
    chat: &crate::Chat,
    node: i64,
    request: &str,
    message: &str,
    target: &crate::chat_push::Target,
    identity: &str,
) -> Result<()> {
    mint(
        conn,
        &NewPushGrant {
            chat_id: chat.id,
            request_id: request.into(),
            node_id: Some(node),
            draft: None,
            mode: chat.mode,
            message: message.into(),
            workspace_path: target.workspace_path.clone(),
            workspace_epoch: target.workspace_epoch,
            branch: target.branch.clone(),
            origin_url: target.origin_url.clone(),
            head_sha: target.head_sha.clone(),
            allow_commit: target.allow_commit,
            supervisor_pid: i64::from(std::process::id()),
            supervisor_identity: identity.into(),
            commit_sha: None,
        },
    )?;
    Ok(())
}

pub(in crate::store) fn mint(tx: &Connection, new: &NewPushGrant) -> Result<i64> {
    let input_message = new.message.trim();
    if let Some((id, message)) = tx
        .query_row(
            "SELECT id,message FROM chat_push_grant WHERE chat_id=?1 AND request_id=?2",
            params![new.chat_id, new.request_id],
            |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if message != input_message {
            return Err(Error::invalid(
                "that push request id belongs to another message",
            ));
        }
        return Ok(id);
    }
    if classify(input_message)
        != (Intent::Authorize {
            commit: new.allow_commit,
        })
        || new.request_id.is_empty()
        || new.request_id.len() > 128
    {
        return Err(Error::invalid(
            "push authority needs an exact direct human instruction and request id",
        ));
    }
    let (mode,active,identity,workspace,archived,stopping):(ChatMode,Option<i64>,Option<String>,String,bool,bool)=tx.query_row("SELECT mode,active_node_id,supervisor_identity,workspace_path,archived,stop_requested FROM chat WHERE id=?1",[new.chat_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    if archived || stopping || mode != new.mode || workspace != new.workspace_path {
        return Err(Error::invalid(
            "this chat changed; send the push instruction again",
        ));
    }
    check_epoch(tx, new.chat_id, new.workspace_epoch)?;
    match (mode, new.node_id, &new.draft) {
        (ChatMode::Single, Some(node), None) => {
            let bound:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM chat_turn t JOIN run r ON r.id=t.run_id JOIN node_run n ON n.id=t.node_id WHERE t.chat_id=?1 AND t.node_id=?2 AND t.request_id=?3 AND r.prompt=?4 AND n.status='running' AND n.supervisor_pid=?5)",params![new.chat_id,node,new.request_id,input_message,new.supervisor_pid],|r|r.get(0))?;
            if !bound
                || active != Some(node)
                || identity.as_deref() != Some(&new.supervisor_identity)
            {
                return Err(Error::invalid(
                    "that exact direct turn no longer owns this chat",
                ));
            }
        }
        (ChatMode::Team, None, Some(target)) => {
            super::super::chat_checkout::available(tx, new.chat_id, None)?;
            draft_matches(tx, new.chat_id, target, &new.branch, &new.head_sha)?;
        }
        _ => {
            return Err(Error::invalid(
                "push authority must name one solo turn or one exact team draft",
            ))
        }
    }
    tx.execute("INSERT INTO chat_push_grant(chat_id,request_id,node_id,draft_json,mode,message,workspace_path,workspace_epoch,branch,origin_url,head_sha,allow_commit,supervisor_pid,supervisor_identity,commit_sha,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?17)",params![new.chat_id,new.request_id,new.node_id,new.draft.as_ref().map(serde_json::to_string).transpose()?,new.mode,input_message,new.workspace_path,new.workspace_epoch,new.branch,new.origin_url,new.head_sha,new.allow_commit,new.supervisor_pid,new.supervisor_identity,new.commit_sha,if new.commit_sha.is_some(){"armed"}else{"pending"},crate::now()])?;
    let id = tx.last_insert_rowid();
    tick(tx, new.chat_id)?;
    Ok(id)
}
fn draft_matches(
    conn: &Connection,
    chat: i64,
    target: &crate::chat_changes::DraftTarget,
    branch: &str,
    commit: &str,
) -> Result<()> {
    let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice s JOIN chat_team_run t ON t.run_id=s.run_id WHERE t.chat_id=?1 AND s.run_id=?2 AND s.slice_key=?3 AND s.rev=?4 AND s.branch=?5 AND s.commit_sha=?6 AND s.build_status='verified' AND s.lease_state='released' AND NOT EXISTS(SELECT 1 FROM chat_build_slice newer JOIN chat_team_run nt ON nt.run_id=newer.run_id WHERE nt.chat_id=?1 AND newer.planner_slice_id=s.planner_slice_id AND newer.run_id>s.run_id))",params![chat,target.run_id,target.slice_key,target.revision,branch,commit],|r|r.get(0))?;
    if !valid {
        return Err(Error::invalid(
            "the exact reviewed draft changed or has a newer attempt; review it again",
        ));
    }
    Ok(())
}
pub(in crate::store) fn check_operation(
    conn: &Connection,
    chat: i64,
    operation: i64,
) -> Result<()> {
    let grant = conn
        .query_row(
            &format!("{SELECT} WHERE chat_id=?1 AND operation_id=?2 AND {STILL_HOLDS}"),
            params![chat, operation],
            from_row,
        )
        .optional()?
        .ok_or_else(|| Error::invalid("this operation's chat push authority was revoked"))?;
    if !grant.ours() {
        return Err(Error::invalid(
            "another process cannot consume this push authority",
        ));
    }
    if let Some(target) = &grant.draft {
        draft_matches(conn, chat, target, &grant.branch, &grant.head_sha)?;
    }
    Ok(())
}
