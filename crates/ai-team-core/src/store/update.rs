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
    let mut query = conn.prepare(
        "SELECT 'chat ' || c.id || ': active turn' FROM chat c WHERE c.active_node_id IS NOT NULL
         AND NOT EXISTS (
           SELECT 1 FROM chat_team_run t JOIN run r ON r.id=t.run_id
           JOIN node_run n ON n.id=t.control_node_id AND n.run_id=t.run_id
           WHERE t.chat_id=c.id AND t.control_node_id=c.active_node_id AND c.mode='team'
             AND t.phase='awaiting_approval' AND t.controller_protocol=1 AND t.child_journal=1 AND t.quiescent=1
             AND t.supervisor_pid IS NULL AND t.supervisor_identity IS NULL
             AND r.status='blocked' AND r.supervisor_pid IS NULL AND n.status='done'
             AND c.stop_requested=0 AND c.supervisor_identity IS NULL AND c.pi_identity IS NULL
             AND NOT EXISTS(SELECT 1 FROM node_run m WHERE m.run_id=t.run_id
               AND (m.status NOT IN ('done','failed','cancelled') OR m.pi_pid IS NOT NULL OR m.supervisor_pid IS NOT NULL))
             AND NOT EXISTS(SELECT 1 FROM chat_build_slice s WHERE s.run_id=t.run_id)
         )
         UNION ALL SELECT 'run ' || id || ': ' || status FROM run WHERE status IN ('queued','planning','running')
         UNION ALL SELECT 'agent turn ' || id || ': ' || status FROM node_run WHERE status IN ('queued','running','parked')
         UNION ALL SELECT 'undrained chat children' WHERE EXISTS(SELECT 1 FROM chat_child WHERE state != 'drained')
         UNION ALL SELECT 'undrained checkout children' WHERE EXISTS(SELECT 1 FROM checkout_child WHERE state != 'drained')
         UNION ALL SELECT 'undrained workspace setup children' WHERE EXISTS(SELECT 1 FROM workspace_setup_child WHERE state != 'drained')
         UNION ALL SELECT 'workspace setup ' || id || ': ' || state FROM workspace_setup WHERE state IN ('pending','running','inspection')
         UNION ALL SELECT 'unfinished delivery' WHERE EXISTS(SELECT 1 FROM chat_delivery WHERE state IN ('running','inspection'))
         UNION ALL SELECT 'unfinished checkout operation' WHERE EXISTS(SELECT 1 FROM chat_checkout_operation WHERE state IN ('running','inspection'))
         UNION ALL SELECT 'unfinished toolbox setup' WHERE EXISTS(SELECT 1 FROM toolbox_operation WHERE state IN ('applying','partial'))
         UNION ALL SELECT 'queued follow-ups' WHERE EXISTS(SELECT 1 FROM chat_followup WHERE state IN ('queued','starting'))
         UNION ALL SELECT 'pending publication' WHERE EXISTS(SELECT 1 FROM chat_push_grant WHERE state IN ('pending','armed'))
         UNION ALL SELECT 'claimed schedule' WHERE EXISTS(SELECT 1 FROM chat_schedule_occurrence WHERE outcome='claimed')
         LIMIT 20",
    )?;
    let blockers = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !blockers.is_empty() {
        return Err(Error::invalid(format!("finish or inspect active work, queued follow-ups and workspace setup before updating; no unreviewed work was stopped. Waiting for: {}", blockers.join("; "))));
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
        let supported = crate::latest_schema();
        if version > supported {
            return Err(Error::invalid(format!(
                "database schema v{version} is newer than this build supports (v{supported}); install a compatible build before continuing. Keep the existing database; initialization cannot repair this mismatch. No data was changed by the updater"
            )));
        }
        if version < supported {
            return Err(Error::invalid("open the current AI Team app to initialize its data before updating; no data was migrated by the updater"));
        }
        Ok(())
    }

    pub fn check_update_idle(&self) -> Result<()> {
        idle(self.db().conn())
    }

    /// Preflight must not migrate, recover or otherwise mutate live state.
    pub fn check_update_idle_at(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        idle(&conn)
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
    fn a_newer_schema_needs_a_compatible_build_not_initialization() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("team.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY)")
            .unwrap();
        let future = crate::latest_schema() + 1;
        conn.execute("INSERT INTO schema_migrations VALUES(?1)", [future])
            .unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();
        let error = Store::check_update_schema(&path).unwrap_err().to_string();
        assert!(error.contains(&format!("schema v{future}")), "{error}");
        assert!(error.contains("compatible build"), "{error}");
        assert!(!error.contains("initialize"), "{error}");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

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
