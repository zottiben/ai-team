//! Upgrade both terminal layouts that shipped under development schema 47.
use std::collections::BTreeSet;

use rusqlite::Transaction;

use crate::{Error, Result};

fn legacy(tx: &Transaction<'_>, table: &str, original: &[&str], added: &[&str]) -> Result<bool> {
    let mut query = tx.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let columns = query
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let old: BTreeSet<String> = original.iter().map(|name| (*name).into()).collect();
    if columns == old {
        return Ok(true);
    }
    let current: BTreeSet<String> = original
        .iter()
        .chain(added)
        .map(|name| (*name).into())
        .collect();
    if columns == current {
        return Ok(false);
    }
    Err(Error::invalid(format!(
        "migration v50 cannot safely identify {table}'s layout; the database is unchanged"
    )))
}

pub(super) fn migrate(tx: &Transaction<'_>, sql: &str) -> Result<()> {
    let pad_legacy = legacy(
        tx,
        "terminal_pad",
        &[
            "id",
            "project_id",
            "position",
            "label",
            "command",
            "created_at",
            "updated_at",
        ],
        &["revision"],
    )?;
    let command_legacy = legacy(
        tx,
        "terminal_command",
        &[
            "id",
            "project_id",
            "token",
            "worktree",
            "label",
            "command",
            "session_id",
            "started_at",
            "finished_at",
            "exit_code",
            "duration_ms",
            "lines",
            "lines_exact",
        ],
        &[
            "request_id",
            "fingerprint",
            "pad_id",
            "revision",
            "dispatch",
            "refusal",
            "claimed_at",
        ],
    )?;
    // Do not lose operator-added indexes or triggers. Restore them only after copying
    // rows, so the migration itself cannot fire an audit/update trigger.
    let objects = tx.prepare("SELECT sql FROM sqlite_master WHERE tbl_name IN ('terminal_pad','terminal_command') AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tx.execute_batch(sql)?;
    let revision = if pad_legacy { "1" } else { "revision" };
    tx.execute_batch(&format!("INSERT INTO terminal_pad_next(id,project_id,position,revision,label,command,created_at,updated_at)
        SELECT id,project_id,position,{revision},label,command,created_at,updated_at FROM terminal_pad"))?;
    // Old receipts prove a spawn, not a client request, pad revision or claim time.
    // The reserved colon-bearing ID cannot pass request_id validation; zero IDs /
    // revisions and an empty claim timestamp preserve those facts as unrecorded.
    let identity = if command_legacy {
        "'legacy:' || id, '', token, worktree, 0, 0, label, command, 'started', NULL, session_id, ''"
    } else {
        "request_id, fingerprint, token, worktree, pad_id, revision, label, command, dispatch, refusal, session_id, claimed_at"
    };
    tx.execute_batch(&format!("INSERT INTO terminal_command_next(id,project_id,request_id,fingerprint,token,worktree,pad_id,revision,label,command,dispatch,refusal,session_id,claimed_at,started_at,finished_at,exit_code,duration_ms,lines,lines_exact)
        SELECT id,project_id,{identity},started_at,finished_at,exit_code,duration_ms,lines,lines_exact FROM terminal_command"))?;
    tx.execute_batch(
        "DROP TABLE terminal_command; DROP TABLE terminal_pad;
        ALTER TABLE terminal_pad_next RENAME TO terminal_pad;
        ALTER TABLE terminal_command_next RENAME TO terminal_command;",
    )?;
    for object in objects {
        tx.execute_batch(&object)?;
    }
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS event_visible_node ON event(node_run_id, id DESC)
    WHERE telemetry_json IS NULL OR payload_json IS NOT NULL",
    )?;
    Ok(())
}
