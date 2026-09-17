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
