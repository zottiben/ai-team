//! Explicit, ai-team-owned engine storage. Never call the neighbour's default-path APIs.

use std::path::{Path, PathBuf};

use ai_planner_core as planner;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{Chat, Error, Result};

const APPLICATION_ID: i64 = 0x4154_504c;

pub(super) fn path(team_db: &Path) -> Result<PathBuf> {
    if team_db == Path::new(":memory:") {
        return Err(Error::invalid(
            "chat planning requires a persistent team database",
        ));
    }
    let db = team_db.canonicalize()?;
    let file = db
        .file_name()
        .ok_or_else(|| Error::invalid("the team database needs a filename"))?;
    let mut name = file.to_os_string();
    name.push(".planning.sqlite");
    Ok(db.with_file_name(name))
}

fn read_owned(path: &Path) -> Result<Connection> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::invalid(
            "the embedded planner store must not be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(Error::invalid(
                "the embedded planner store must not be a shared hard link",
            ));
        }
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let id: i64 = conn.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if id != APPLICATION_ID {
        return Err(Error::invalid(
            "this file is not an ai-team-owned planning store",
        ));
    }
    let objects: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE
         (type = 'trigger' AND name GLOB 'ai_team_*') OR
         (type = 'table' AND name = 'ai_team_plan_revision')",
        [],
        |row| row.get(0),
    )?;
    if objects != 25 {
        return Err(Error::invalid(
            "the embedded planning revision schema is incomplete",
        ));
    }
    Ok(conn)
}

pub(super) fn open(path: &Path, create: bool) -> Result<Option<planner::Store>> {
    if path.try_exists()? || std::fs::symlink_metadata(path).is_ok() {
        read_owned(path)?;
        return Ok(Some(planner::Store::open(path)?));
    }
    if !create {
        return Ok(None);
    }
    // Publish only a complete, checkpointed database. A crash during initialization
    // leaves at most an unused temporary file, never an owned-but-partial sidecar.
    let staged = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| Error::invalid("the planner store needs a parent directory"))?,
    )?;
    let store = planner::Store::init(staged.path())?;
    initialize(&store)?;
    store
        .db()
        .conn()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(store);
    staged
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    read_owned(path)?;
    Ok(Some(planner::Store::open(path)?))
}

fn initialize(store: &planner::Store) -> Result<()> {
    let tx = store.db().conn().unchecked_transaction()?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.execute_batch(
        "CREATE TABLE ai_team_plan_revision (plan_id INTEGER PRIMARY KEY, rev INTEGER NOT NULL)",
    )?;
    // Engine commits advance the cursor atomically, even if the process dies before
    // returning to ai-team. No timer/mtime heuristic and no copied plan in team.db.
    for (table, id) in [
        ("plan", "id"),
        ("plan_section", "plan_id"),
        ("slice", "plan_id"),
        ("question", "plan_id"),
        ("decision", "plan_id"),
        ("gotcha", "plan_id"),
        ("log", "plan_id"),
        ("plan_source", "plan_id"),
    ] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let row = if action == "DELETE" { "OLD" } else { "NEW" };
            tx.execute_batch(&format!(
                "CREATE TRIGGER ai_team_{table}_{action} AFTER {action} ON {table} BEGIN
                   INSERT INTO ai_team_plan_revision (plan_id, rev) VALUES ({row}.{id}, 1)
                   ON CONFLICT(plan_id) DO UPDATE SET rev = rev + 1;
                 END;"
            ))?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn revision(conn: &Connection, plan: i64) -> Result<i64> {
    conn.query_row(
        "SELECT rev FROM ai_team_plan_revision WHERE plan_id = ?1",
        [plan],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| Error::invalid("the embedded plan has no revision cursor"))
}

pub(super) fn total_revision(path: &Path) -> Result<i64> {
    if !path.try_exists()? {
        return Ok(0);
    }
    Ok(read_owned(path)?.query_row(
        "SELECT COALESCE(SUM(rev), 0) FROM ai_team_plan_revision",
        [],
        |row| row.get(0),
    )?)
}

pub(super) fn find(store: &planner::Store, chat: &Chat) -> Result<Option<planner::Plan>> {
    let Some(repo) = store.find_repo(&repo_key(chat))? else {
        return Ok(None);
    };
    // The engine's human-facing find_plan is deliberately fuzzy and may fall back
    // across repos. Ownership must instead use exact repo + chat identity.
    let id: Option<i64> = store
        .db()
        .conn()
        .query_row(
            "SELECT id FROM plan WHERE repo_id = ?1 AND slug = ?2",
            rusqlite::params![repo.id, format!("chat-{}", chat.id)],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| store.get_plan(id).map_err(Into::into))
        .transpose()
}

pub(super) fn repo_key(chat: &Chat) -> String {
    format!("ai-team-project-{}", chat.project_id)
}
