//! The store: the only thing that writes SQL.
//!
//! Split by subject rather than kept in one file, but it is one type - `Store` - so a
//! caller never has to know which module a method lives in. Nothing above this crate
//! writes SQL; that is what keeps the CLI, the server and the desktop shell from
//! inventing three slightly different ideas of what a run is.

mod agents;
mod events;
mod notifications;
mod projects;
mod reminders;
mod reviews;
mod runs;

use std::path::Path;

use crate::db::Db;
use crate::error::Result;

#[derive(Debug)]
pub struct Store {
    db: Db,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Ok(Store {
            db: Db::open(path)?,
        })
    }

    /// Create the database if it is not there, and migrate it. `ait init` calls this;
    /// everything else calls [`Store::open`], so a wrong path is an error rather than a
    /// second empty database.
    pub fn init(path: &Path) -> Result<Store> {
        Ok(Store {
            db: Db::open_or_create(path)?,
        })
    }

    pub fn open_default() -> Result<Store> {
        Ok(Store {
            db: Db::open_default()?,
        })
    }

    pub fn memory() -> Result<Store> {
        Ok(Store { db: Db::memory()? })
    }

    pub fn path(&self) -> &Path {
        self.db.path()
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.db.schema_version()
    }

    /// The `v_` views this database actually has, read from `sqlite_master` rather than
    /// from a list in the code - a hardcoded list would drift from the schema that
    /// shipped, which is precisely the thing it would be claiming to report.
    pub fn views(&self) -> Result<Vec<String>> {
        let mut stmt = self.db.conn().prepare(
            "SELECT name FROM sqlite_master WHERE type = 'view' AND name LIKE 'v_%' ORDER BY name",
        )?;
        let names = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(names)
    }

    pub(crate) fn db(&self) -> &Db {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut Db {
        &mut self.db
    }
}

/// `rusqlite` hands back `Option<String>` for a nullable TEXT, but an empty string is
/// not the same as NULL and the two get confused constantly. This makes the intent
/// explicit at the call site.
pub(crate) fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// Something said to a seat that was busy at the time (M9-S42).
impl Store {
    /// Keep a legacy seat-wide message for its next turn.
    ///
    /// New interactive paths use [`Store::queue_node_message`]. Keeping this form lets a
    /// database upgraded from schema 10 deliver rows that did not yet know their node.
    #[cfg(test)]
    pub(crate) fn queue_legacy_message(&mut self, agent_id: i64, body: &str) -> Result<usize> {
        let at = crate::util::now();
        self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO pending_message (agent_id, body, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![agent_id, body, at],
            )?;
            Ok(())
        })?;
        self.waiting_for(agent_id)
    }

    /// Keep a message for one exact live conversation.
    pub fn queue_node_message(
        &mut self,
        node_run_id: i64,
        agent_id: i64,
        body: &str,
    ) -> Result<usize> {
        let node = self.node_run(node_run_id)?;
        if node.agent_id != Some(agent_id) {
            return Err(crate::error::Error::invalid(
                "that agent does not own that node",
            ));
        }
        let at = crate::util::now();
        self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO pending_message (agent_id, node_run_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![agent_id, node_run_id, body, at],
            )?;
            Ok(())
        })?;
        self.waiting_for_node(agent_id, node_run_id)
    }

    /// Keep a reply in a node's visible conversation and in that node's delivery queue.
    ///
    /// One transaction is essential: a reply must never be visible but undeliverable, or
    /// queued while absent from the transcript the person is looking at.
    pub fn queue_conversation(
        &mut self,
        node_run_id: i64,
        agent_id: i64,
        body: &str,
    ) -> Result<usize> {
        let node = self.node_run(node_run_id)?;
        if node.agent_id != Some(agent_id) {
            return Err(crate::error::Error::invalid(
                "that agent does not own that node",
            ));
        }
        let body = body.trim();
        if body.is_empty() {
            return Err(crate::error::Error::invalid("say something"));
        }
        let at = crate::util::now();
        let summary = conversation_summary(body);
        let payload = serde_json::to_string(&serde_json::json!({
            "conversation": "reply",
            "body": body,
        }))?;
        let waiting: i64 = self.db_mut().write(|tx| {
            tx.execute(
                "INSERT INTO pending_message (agent_id, node_run_id, body, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![agent_id, node_run_id, body, at],
            )?;
            tx.execute(
                "INSERT INTO event (run_id, node_run_id, at, kind, actor, summary, payload_json)
                 VALUES (?1, ?2, ?3, 'note', 'human', ?4, ?5)",
                rusqlite::params![node.run_id, node_run_id, at, summary, payload],
            )?;
            Ok(tx.query_row(
                "SELECT COUNT(*) FROM pending_message
                  WHERE agent_id = ?1 AND node_run_id = ?2 AND delivered_at IS NULL",
                rusqlite::params![agent_id, node_run_id],
                |row| row.get(0),
            )?)
        })?;
        Ok(usize::try_from(waiting).unwrap_or(0))
    }

    /// How many messages this seat has not been given yet, across old and current rows.
    pub fn waiting_for(&self, agent_id: i64) -> Result<usize> {
        let count: i64 = self.db().conn().query_row(
            "SELECT COUNT(*) FROM pending_message
              WHERE agent_id = ?1 AND delivered_at IS NULL",
            rusqlite::params![agent_id],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// How many replies belong to this exact node. Legacy unscoped rows are included so
    /// an upgrade does not strand something a person already said.
    pub fn waiting_for_node(&self, agent_id: i64, node_run_id: i64) -> Result<usize> {
        let count: i64 = self.db().conn().query_row(
            "SELECT COUNT(*) FROM pending_message
              WHERE agent_id = ?1 AND (node_run_id = ?2 OR node_run_id IS NULL)
                AND delivered_at IS NULL",
            rusqlite::params![agent_id, node_run_id],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Take all legacy messages waiting for a seat.
    #[cfg(test)]
    pub(crate) fn take_legacy_pending(&mut self, agent_id: i64) -> Result<Vec<String>> {
        self.take_pending_where(agent_id, None)
    }

    /// Take replies for this node only, plus legacy rows that predate node scoping.
    pub fn take_pending_for(&mut self, agent_id: i64, node_run_id: i64) -> Result<Vec<String>> {
        self.take_pending_where(agent_id, Some(node_run_id))
    }

    fn take_pending_where(
        &mut self,
        agent_id: i64,
        node_run_id: Option<i64>,
    ) -> Result<Vec<String>> {
        let at = crate::util::now();
        self.db_mut().write(|tx| {
            let sql = match node_run_id {
                Some(_) => {
                    "SELECT id, body FROM pending_message
                      WHERE agent_id = ?1 AND (node_run_id = ?2 OR node_run_id IS NULL)
                        AND delivered_at IS NULL ORDER BY id"
                }
                None => {
                    "SELECT id, body FROM pending_message
                      WHERE agent_id = ?1 AND node_run_id IS NULL
                        AND delivered_at IS NULL ORDER BY id"
                }
            };
            let mut read = tx.prepare(sql)?;
            let rows: Vec<(i64, String)> = match node_run_id {
                Some(node_run_id) => read
                    .query_map(rusqlite::params![agent_id, node_run_id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<std::result::Result<_, _>>()?,
                None => read
                    .query_map(rusqlite::params![agent_id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<std::result::Result<_, _>>()?,
            };
            drop(read);

            for (id, _) in &rows {
                tx.execute(
                    "UPDATE pending_message SET delivered_at = ?2 WHERE id = ?1",
                    rusqlite::params![id, at],
                )?;
            }
            Ok(rows.into_iter().map(|(_, body)| body).collect())
        })
    }
}

fn conversation_summary(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 160 {
        return flat;
    }
    flat.chars().take(159).collect::<String>() + "…"
}

impl Store {
    /// Where a project's Pi support files live: the guard, and one MCP config per seat.
    ///
    /// Deliberately outside every lease. A guard or an allow-list a node can edit is not
    /// one, and a lease is precisely the directory a node may write to.
    pub fn support_dir(&self, project_slug: &str) -> Result<std::path::PathBuf> {
        Ok(crate::data_dir()?.join("seats").join(project_slug))
    }
}
