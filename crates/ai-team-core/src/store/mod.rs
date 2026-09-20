//! The store: the only thing that writes SQL.
//!
//! Split by subject rather than kept in one file, but it is one type - `Store` - so a
//! caller never has to know which module a method lives in. Nothing above this crate
//! writes SQL; that is what keeps the CLI, the server and the desktop shell from
//! inventing three slightly different ideas of what a run is.

mod agents;
mod events;
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
    /// Keep a message for a seat's next turn.
    ///
    /// Returns how many are now waiting, so the caller can say "it will get this after
    /// what it is doing" rather than pretending it has already landed.
    pub fn queue_message(&mut self, agent_id: i64, body: &str) -> Result<usize> {
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

    /// How many messages this seat has not been given yet.
    pub fn waiting_for(&self, agent_id: i64) -> Result<usize> {
        let count: i64 = self.db().conn().query_row(
            "SELECT COUNT(*) FROM pending_message
              WHERE agent_id = ?1 AND delivered_at IS NULL",
            rusqlite::params![agent_id],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Take everything waiting for a seat, oldest first, and mark it delivered.
    ///
    /// Marked in the same transaction as the read. Two turns starting at once would
    /// otherwise both take the same message and act on it twice, which for an instruction
    /// like "stop adding tests" is worse than not delivering it at all.
    pub fn take_pending(&mut self, agent_id: i64) -> Result<Vec<String>> {
        let at = crate::util::now();
        self.db_mut().write(|tx| {
            let mut read = tx.prepare(
                "SELECT id, body FROM pending_message
                  WHERE agent_id = ?1 AND delivered_at IS NULL
                  ORDER BY id",
            )?;
            let rows: Vec<(i64, String)> = read
                .query_map(rusqlite::params![agent_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
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
