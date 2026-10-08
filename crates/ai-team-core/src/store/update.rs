//! A short database fence around installation, not a cancellation mechanism.

use crate::{Error, Result, Store};
use rusqlite::{Connection, OpenFlags, OptionalExtension, MAIN_DB};
use std::path::Path;

pub(crate) fn check_writer(conn: &Connection) -> Result<()> {
    let installed: Option<String> = conn
        .query_row("SELECT version FROM app_update WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()?;
    if installed.is_some_and(|v| crate::is_newer(&v, crate::current_version())) {
        return Err(Error::invalid(
            "AI Team was updated; restart this process before continuing",
        ));
    }
    Ok(())
}

fn idle(conn: &Connection) -> Result<()> {
    let busy: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat WHERE active_node_id IS NOT NULL)
         OR EXISTS(SELECT 1 FROM run WHERE status IN ('queued','planning','running'))
         OR EXISTS(SELECT 1 FROM node_run WHERE status IN ('queued','running','parked'))
         OR EXISTS(SELECT 1 FROM chat_child WHERE state != 'drained')
         OR EXISTS(SELECT 1 FROM checkout_child WHERE state != 'drained')
         OR EXISTS(SELECT 1 FROM workspace_setup_child WHERE state != 'drained')
         OR EXISTS(SELECT 1 FROM workspace_setup WHERE state IN ('pending','running','inspection'))
         OR EXISTS(SELECT 1 FROM chat_delivery WHERE state IN ('running','inspection'))
         OR EXISTS(SELECT 1 FROM chat_checkout_operation WHERE state IN ('running','inspection'))
         OR EXISTS(SELECT 1 FROM toolbox_operation WHERE state IN ('applying','partial'))
         OR EXISTS(SELECT 1 FROM chat_followup WHERE state IN ('queued','starting'))
         OR EXISTS(SELECT 1 FROM chat_push_grant WHERE state IN ('pending','armed'))
         OR EXISTS(SELECT 1 FROM chat_schedule_occurrence WHERE outcome='claimed')",
        [],
        |r| r.get(0),
    )?;
    if busy {
        return Err(Error::invalid("finish or inspect active work, queued follow-ups and workspace setup before updating; no work was stopped"));
    }
    Ok(())
}

impl Store {
    /// Checking/updating the installation must not quietly migrate live data before its backup.
    pub fn check_update_schema(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if version != crate::latest_schema() {
            return Err(Error::invalid("open the current AI Team app to initialize its data before updating; no data was migrated by the updater"));
        }
        Ok(())
    }

    pub fn check_update_idle(&self) -> Result<()> {
        idle(self.db().conn())
    }

    pub fn installed_application_version(path: &Path) -> Result<Option<String>> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let registered: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_update')",
            [],
            |r| r.get(0),
        )?;
        if !registered {
            return Ok(None);
        }
        Ok(conn
            .query_row("SELECT version FROM app_update WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Called only after inspection matched every installed byte to the prepared receipt.
    /// Do not migrate, dispatch work, replay installation or restore data during recovery.
    pub fn record_inspected_application_update(
        path: &Path,
        version: &str,
        backup: &Path,
    ) -> Result<()> {
        let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        idle(&tx)?;
        record(&tx, version, backup)?;
        tx.commit()?;
        Ok(())
    }

    /// Backup both owned databases under the same admission fence as the final swap.
    /// Other processes cannot admit work between this check and installing the files.
    pub fn install_application_update<T>(
        &mut self,
        version: &str,
        backup: &Path,
        install: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let db = self.path().to_path_buf();
        let planner = if db == Path::new(":memory:") {
            None
        } else {
            Some(self.planning_path()?)
        };
        self.db_mut().write_update(|tx| {
            idle(tx)?;
            if db != Path::new(":memory:") {
                snapshot(&db, &backup.join("team.db"))?;
                if let Some(planner) = planner.as_ref().filter(|p| p.exists()) {
                    snapshot(planner, &backup.join("team.db.planning.sqlite"))?;
                }
            }
            let result = install()?;
            record(tx, version, backup)?;
            Ok(result)
        })
    }
}

fn record(conn: &Connection, version: &str, backup: &Path) -> Result<()> {
    conn.execute("INSERT INTO app_update(id,version,backup_path,installed_at) VALUES(1,?1,?2,?3)
        ON CONFLICT(id) DO UPDATE SET version=excluded.version,backup_path=excluded.backup_path,installed_at=excluded.installed_at",
        rusqlite::params![version, backup.to_string_lossy(), crate::now()])?;
    Ok(())
}

fn snapshot(source: &Path, target: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.backup(MAIN_DB, target, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_fences_future_writes_without_mutating_existing_evidence() {
        let mut store = Store::memory().unwrap();
        let backup = tempfile::tempdir().unwrap();
        store
            .install_application_update("999.0.0", backup.path(), || Ok(()))
            .unwrap();
        let error = store.db_mut().write(|_| Ok(())).unwrap_err().to_string();
        assert!(error.contains("restart"), "{error}");
        assert_eq!(store.schema_version().unwrap(), crate::latest_schema());
    }

    #[test]
    fn a_failed_swap_does_not_record_an_installed_version() {
        let mut store = Store::memory().unwrap();
        let backup = tempfile::tempdir().unwrap();
        assert!(store
            .install_application_update("999.0.0", backup.path(), || Err::<(), _>(Error::invalid(
                "fixture failure"
            )))
            .is_err());
        store.db_mut().write(|_| Ok(())).unwrap();
    }
}
