//! The connection, the pragmas and the migration runner.
//!
//! Every write in the crate goes through [`Db::write`], and every schema change is a new
//! entry in [`MIGRATIONS`]. Migrations are `include_str!`d rather than read from disk for
//! the same reason the frontend is: a binary installed with `cargo install` has no repo
//! next to it, and a schema it cannot find is a schema it cannot apply.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::{Error, Result};
use crate::util::now;

/// The schema version this build carries.
///
/// Derived from [`MIGRATIONS`] rather than written down, because a constant somebody has
/// to remember to bump is a constant that goes stale the one time it matters - when a
/// database is a migration behind and nothing says so.
pub fn latest_schema() -> i64 {
    MIGRATIONS.last().map_or(0, |(version, _, _)| *version)
}

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "core", include_str!("migrations/001_core.sql")),
    (2, "eve", include_str!("migrations/002_eve.sql")),
    (3, "repairs", include_str!("migrations/003_repairs.sql")),
    (
        4,
        "guardrails",
        include_str!("migrations/004_guardrails.sql"),
    ),
    (5, "console", include_str!("migrations/005_console.sql")),
    (6, "schedule", include_str!("migrations/006_schedule.sql")),
    (7, "speak", include_str!("migrations/007_speak.sql")),
    (
        8,
        "workspace_runs",
        include_str!("migrations/008_workspace_runs.sql"),
    ),
    (
        9,
        "notifications",
        include_str!("migrations/009_notifications.sql"),
    ),
    (
        10,
        "session_context",
        include_str!("migrations/010_session_context.sql"),
    ),
    (
        11,
        "scoped_messages",
        include_str!("migrations/011_scoped_messages.sql"),
    ),
    (12, "delivery", include_str!("migrations/012_delivery.sql")),
    (
        13,
        "supervision",
        include_str!("migrations/013_supervision.sql"),
    ),
];

/// The number of `v_` views the schema ships. Asserted in tests, because a view silently
/// dropped from a migration is a demo that quietly stops working.
#[cfg(test)]
const VIEW_COUNT: i64 = 7;

#[derive(Debug)]
pub struct Db {
    conn: Connection,
    path: PathBuf,
}

impl Db {
    /// Open an existing database. Fails rather than creating one, so a typo in a path
    /// cannot silently produce a second, empty ai-team.
    pub fn open(path: &Path) -> Result<Db> {
        if !path.exists() {
            return Err(Error::NoDatabase(path.to_path_buf()));
        }
        Db::open_or_create(path)
    }

    pub fn open_or_create(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        configure(&conn)?;
        let mut db = Db {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_default() -> Result<Db> {
        Db::open(&crate::default_db_path()?)
    }

    pub fn memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        configure(&conn)?;
        let mut db = Db {
            conn,
            path: PathBuf::from(":memory:"),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Every write goes through here. `IMMEDIATE` takes the write lock up front, so a
    /// concurrent writer waits on `busy_timeout` instead of failing halfway through a
    /// transaction with `SQLITE_BUSY`.
    pub fn write<T>(&mut self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version    INTEGER PRIMARY KEY,
                 name       TEXT NOT NULL,
                 applied_at TEXT NOT NULL
             )",
        )?;
        let applied: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        let latest = latest_schema();
        if applied > latest {
            return Err(Error::invalid(format!(
                "database schema v{applied} is newer than this ai-team build (v{latest}); update ai-team before opening it"
            )));
        }

        for (version, name, sql) in MIGRATIONS {
            if *version <= applied {
                continue;
            }
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![version, name, now()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn pending_migrations(&self) -> Result<i64> {
        let latest = MIGRATIONS.iter().map(|(v, _, _)| *v).max().unwrap_or(0);
        Ok(latest - self.schema_version()?)
    }
}

fn configure(conn: &Connection) -> Result<()> {
    // WAL lets readers run while a writer holds the lock, which is the normal state when
    // several nodes are busy. `busy_timeout` turns a lock collision into a short wait
    // rather than an error every call site would have to handle.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_run(tx: &Transaction<'_>) -> Result<()> {
        tx.execute(
            "INSERT INTO project (id, slug, name, created_at, updated_at)
             VALUES (1, 'p', 'P', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )?;
        tx.execute(
            "INSERT INTO run (id, project_id, prompt, created_at, updated_at)
             VALUES (1, 1, 'do the thing', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )?;
        Ok(())
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("team.db");
        let db = Db::open_or_create(&path).unwrap();
        let newer = latest_schema() + 1;
        db.conn()
            .execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', ?2)",
                rusqlite::params![newer, now()],
            )
            .unwrap();
        drop(db);

        let error = Db::open(&path).unwrap_err();
        assert!(error.to_string().contains("newer"), "{error}");
        assert!(error.to_string().contains(&format!("v{newer}")), "{error}");
    }

    #[test]
    fn migrations_are_idempotent_and_create_the_views() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("team.db");

        // Derived from the list rather than written down twice: a constant here is one
        // somebody has to remember to bump, and forgetting reads as a failed migration.
        let latest = MIGRATIONS.last().expect("at least one migration").0;

        let db = Db::open_or_create(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), latest);
        assert_eq!(db.pending_migrations().unwrap(), 0);
        drop(db);

        // Re-opening must not re-apply anything.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), latest);

        let views: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name LIKE 'v_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            views, VIEW_COUNT,
            "the TablePlus views must ship with the schema"
        );
    }

    #[test]
    fn workspace_migration_does_not_guess_a_home_for_old_zero_node_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("team.db");
        let conn = Connection::open(&path).unwrap();
        configure(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at TEXT NOT NULL
             )",
        )
        .unwrap();
        for (version, name, sql) in MIGRATIONS.iter().take(7) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO project (id, slug, name, created_at, updated_at)
             VALUES (1, 'p', 'P', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO project_repo
                    (id, project_id, key, name, main_path, created_at)
             VALUES (1, 1, 'p', 'P', '/repo/main', '2026-01-01T00:00:00Z');
             INSERT INTO run (id, project_id, prompt, created_at, updated_at)
             VALUES (1, 1, 'never dispatched', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
                    (2, 1, 'planned in task', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO node_run
                    (run_id, role, provider, model, worktree_path, created_at, updated_at)
             VALUES (2, 'orchestrator', 'local', 'auto', '/repo/task',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        drop(conn);

        let db = Db::open(&path).unwrap();
        let unassigned: Option<String> = db
            .conn()
            .query_row("SELECT workspace_path FROM run WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let task: Option<String> = db
            .conn()
            .query_row("SELECT workspace_path FROM run WHERE id = 2", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(unassigned, None);
        assert_eq!(task.as_deref(), Some("/repo/task"));
    }

    #[test]
    fn opening_a_missing_database_is_an_error_not_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope.db");
        assert!(matches!(Db::open(&path), Err(Error::NoDatabase(_))));
        assert!(!path.exists());
    }

    #[test]
    fn the_event_table_refuses_updates() {
        let mut db = Db::memory().unwrap();
        db.write(|tx| {
            seed_run(tx)?;
            tx.execute(
                "INSERT INTO event (run_id, at, kind, summary)
                 VALUES (1, '2026-01-01T00:00:00Z', 'step', 'first')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let rewritten = db
            .conn()
            .execute("UPDATE event SET summary = 'rewritten' WHERE id = 1", []);
        assert!(rewritten.is_err(), "event rows must not be rewritable");

        let summary: String = db
            .conn()
            .query_row("SELECT summary FROM event WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(summary, "first");
    }

    #[test]
    fn deleting_a_run_still_cascades_to_its_events() {
        // The append-only trigger guards UPDATE only. Deletes have to keep working, or
        // dropping a project would leave orphaned events behind forever.
        let mut db = Db::memory().unwrap();
        db.write(|tx| {
            seed_run(tx)?;
            tx.execute(
                "INSERT INTO event (run_id, at, kind) VALUES (1, '2026-01-01T00:00:00Z', 'step')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        db.write(|tx| {
            tx.execute("DELETE FROM project WHERE id = 1", [])?;
            Ok(())
        })
        .unwrap();

        let events: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
    }

    #[test]
    fn check_constraints_reject_invented_values() {
        let mut db = Db::memory().unwrap();
        db.write(seed_run).unwrap();

        // An invented project kind.
        assert!(db
            .conn()
            .execute(
                "INSERT INTO project (slug, name, kind, created_at, updated_at)
                 VALUES ('x', 'X', 'sprint', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .is_err());

        // A metered provider. D8 is enforced in the schema, not only in the picker.
        db.write(|tx| {
            tx.execute(
                "INSERT INTO team (id, project_id, slug, name, created_at, updated_at)
                 VALUES (1, 1, 't', 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db
            .conn()
            .execute(
                "INSERT INTO agent (team_id, role, name, provider, model, prompt_preset,
                                    created_at, updated_at)
                 VALUES (1, 'backend', 'B', 'anthropic-api', 'claude-opus-5', 'backend',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .is_err());
    }

    #[test]
    fn an_agent_must_have_a_preset_or_a_prompt() {
        let mut db = Db::memory().unwrap();
        db.write(|tx| {
            seed_run(tx)?;
            tx.execute(
                "INSERT INTO team (id, project_id, slug, name, created_at, updated_at)
                 VALUES (1, 1, 't', 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        // An agent with neither is an agent with no instructions at all.
        assert!(db
            .conn()
            .execute(
                "INSERT INTO agent (team_id, role, name, model, created_at, updated_at)
                 VALUES (1, 'backend', 'B', 'glm-4.6', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .is_err());
    }

    #[test]
    fn a_scheduled_run_must_carry_a_project() {
        // 006 replaced the old "must carry a prompt" rule. Since Q14 an empty prompt
        // means "build whatever the plan has ready", which is the most useful thing to
        // schedule - but with no project there is nowhere to lease a worktree and no
        // plan to read, and the failure would come at two in the morning.
        let mut db = Db::memory().unwrap();
        db.write(seed_run).unwrap();

        assert!(db
            .conn()
            .execute(
                "INSERT INTO reminder (kind, title, prompt, created_at, updated_at)
                 VALUES ('scheduled_run', 'nightly', 'tidy the tests',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .is_err());

        // And a prompt is genuinely optional now.
        assert!(db
            .conn()
            .execute(
                "INSERT INTO reminder (project_id, kind, title, created_at, updated_at)
                 VALUES (1, 'scheduled_run', 'nightly', '2026-01-01T00:00:00Z',
                         '2026-01-01T00:00:00Z')",
                [],
            )
            .is_ok());
    }
}
