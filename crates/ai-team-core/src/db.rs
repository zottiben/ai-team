//! The connection, the pragmas and the migration runner.
//!
//! Every write in the crate goes through [`Db::write`], and every schema change is a new
//! entry in [`MIGRATIONS`]. Migrations are `include_str!`d rather than read from disk for
//! the same reason the frontend is: a binary installed with `cargo install` has no repo
//! next to it, and a schema it cannot find is a schema it cannot apply.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

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
    (
        14,
        "run_branch",
        include_str!("migrations/014_run_branch.sql"),
    ),
    (15, "tasks", include_str!("migrations/015_tasks.sql")),
    (16, "restack", include_str!("migrations/016_restack.sql")),
    (
        17,
        "context_from_evidence",
        include_str!("migrations/017_context_from_evidence.sql"),
    ),
    (18, "chats", include_str!("migrations/018_chats.sql")),
    (
        19,
        "chat_teams",
        include_str!("migrations/019_chat_teams.sql"),
    ),
    (
        20,
        "chat_build_approval",
        include_str!("migrations/020_chat_build_approval.sql"),
    ),
    (
        21,
        "chat_build_results",
        include_str!("migrations/021_chat_build_results.sql"),
    ),
    (
        22,
        "chat controller lifetime",
        include_str!("migrations/022_chat_controller_lifetime.sql"),
    ),
    (
        23,
        "chat child process journal",
        include_str!("migrations/023_chat_children.sql"),
    ),
    (
        24,
        "chat build closure",
        include_str!("migrations/024_chat_build_closure.sql"),
    ),
    (
        25,
        "chat delivery",
        include_str!("migrations/025_chat_delivery.sql"),
    ),
    (
        26,
        "toolbox previews",
        include_str!("migrations/026_toolbox.sql"),
    ),
    (
        27,
        "scoped toolbox operations",
        include_str!("migrations/027_toolbox_operations.sql"),
    ),
    (
        28,
        "chat follow-ups",
        include_str!("migrations/028_chat_followups.sql"),
    ),
    (
        30,
        "chat schedules",
        include_str!("migrations/030_chat_schedule.sql"),
    ),
    (
        31,
        "chat checkout actions",
        include_str!("migrations/031_chat_checkout.sql"),
    ),
    (
        32,
        "notification inbox",
        include_str!("migrations/032_notification_inbox.sql"),
    ),
    (
        33,
        "chat worktree requests",
        include_str!("migrations/033_chat_workspaces.sql"),
    ),
    (
        34,
        "AWT workspace setup",
        include_str!("migrations/034_workspace_setup.sql"),
    ),
    (
        35,
        "installed application update",
        include_str!("migrations/035_app_update.sql"),
    ),
    (
        36,
        "explicit chat push authority",
        include_str!("migrations/036_chat_push_grant.sql"),
    ),
    (
        37,
        "submitted review repairs",
        include_str!("migrations/037_chat_review_requests.sql"),
    ),
    (
        38,
        "explicit new-branch push scope",
        include_str!("migrations/038_chat_push_branch.sql"),
    ),
    (
        39,
        "inline untracked review",
        include_str!("migrations/039_untracked_review.sql"),
    ),
    (
        40,
        "app-owned MCP servers",
        include_str!("migrations/040_mcp_servers.sql"),
    ),
    (
        41,
        "chat image input",
        include_str!("migrations/041_chat_images.sql"),
    ),
    (
        42,
        "turn thinking snapshots",
        include_str!("migrations/042_turn_reasoning.sql"),
    ),
    (
        43,
        "full thinking range",
        include_str!("migrations/043_thinking_levels.sql"),
    ),
    (
        44,
        "supervisor event observations",
        include_str!("migrations/044_event_observations.sql"),
    ),
];

/// Migrations that rebuild a table other tables point at.
///
/// SQLite cannot alter a CHECK in place, so widening one means the documented
/// create-copy-drop-rename procedure - and dropping a parent table with foreign keys on
/// would either fail or cascade rows away. These run with enforcement off, which
/// `PRAGMA` refuses to change inside a transaction, so the switch has to happen out here
/// rather than in the script. `foreign_key_check` then has to pass before the commit, or
/// the migration is rolled back with enforcement still intact.
const REBUILDS: &[i64] = &[43];

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
        let _permit = crate::update::fence::shared(path)?;
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
        let _permit = crate::update::fence::shared(&self.path)?;
        self.write_update(f)
    }

    /// Only the updater may write while it owns the exclusive installation fence.
    pub(crate) fn write_update<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        crate::store::update::check_writer(&tx)?;
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

        let old_chats: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 14 AND name = 'chats')",
            [],
            |row| row.get(0),
        )?;
        if old_chats {
            return Err(Error::invalid("this database uses the pre-release chat schema; keep it unchanged and choose a new isolated ai-team database for this build"));
        }

        for (version, name, sql) in MIGRATIONS {
            if *version <= applied {
                continue;
            }
            if REBUILDS.contains(version) {
                self.rebuild(*version, name, sql)?;
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

    /// Apply one table-rebuilding migration, with foreign keys off for its transaction
    /// only.
    ///
    /// The pragmas are restored on every path, including a failed migration and a failed
    /// integrity check, because a connection left with enforcement off is one that
    /// accepts orphan rows for as long as the process lives. `legacy_alter_table` keeps
    /// `ALTER TABLE ... RENAME` from reparsing views and triggers that name a table while
    /// it is mid-rebuild; the script drops and recreates those itself.
    fn rebuild(&mut self, version: i64, name: &str, sql: &str) -> Result<()> {
        self.conn.pragma_update(None, "foreign_keys", false)?;
        let out = self
            .conn
            .pragma_update(None, "legacy_alter_table", true)
            .map_err(Error::from)
            .and_then(|()| self.rebuild_inner(version, name, sql));
        // Attempt both restorations even if either pragma itself fails.
        let legacy = self.conn.pragma_update(None, "legacy_alter_table", false);
        let foreign_keys = self.conn.pragma_update(None, "foreign_keys", true);
        out?;
        legacy?;
        Ok(foreign_keys?)
    }

    fn rebuild_inner(&mut self, version: i64, name: &str, sql: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![version, name, now()],
        )?;
        // Nothing may be left pointing at a row that is no longer there. Reported with
        // the offending table rather than a bare count, because the useful question
        // after this fails is which rebuild dropped what.
        let orphan: Option<String> = tx
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()?;
        if let Some(table) = orphan {
            return Err(Error::invalid(format!(
                "migration v{version} ({name}) left {table} referencing rows that are gone; \
                 the database is unchanged"
            )));
        }
        tx.commit()?;
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
    fn untracked_review_migration_preserves_findings_sequences_and_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE chat(id INTEGER PRIMARY KEY); INSERT INTO chat VALUES(1);").unwrap();
        let (_, finding) = include_str!("migrations/031_chat_checkout.sql")
            .split_once("CREATE TABLE chat_checkout_finding ")
            .unwrap();
        conn.execute_batch(&format!("CREATE TABLE chat_checkout_finding {finding}"))
            .unwrap();
        conn.execute_batch("INSERT INTO chat_checkout_finding VALUES(7,1,'original',NULL,'unstaged','file','new',1,'keep this evidence','then'); INSERT INTO chat_checkout_finding SELECT 99,chat_id,fingerprint,head,area,path,side,line,body,created_at FROM chat_checkout_finding WHERE id=7; DELETE FROM chat_checkout_finding WHERE id=99;").unwrap();
        let rows = || {
            conn.query_row("SELECT json_array(id,chat_id,fingerprint,head,area,path,side,line,body,created_at) FROM chat_checkout_finding WHERE id=7", [], |r| r.get::<_, String>(0)).unwrap()
        };
        let before = rows();
        conn.execute_batch(include_str!("migrations/039_untracked_review.sql"))
            .unwrap();
        assert_eq!(rows(), before);
        conn.execute("INSERT INTO chat_checkout_finding(chat_id,fingerprint,area,path,side,line,body,created_at) VALUES(1,'fresh','untracked','new-file','new',1,'new finding','now')", []).unwrap();
        assert_eq!(conn.last_insert_rowid(), 100);
        assert!(conn
            .execute(
                "UPDATE chat_checkout_finding SET body='rewritten' WHERE id=7",
                []
            )
            .is_err());
        assert!(conn.execute("INSERT INTO chat_checkout_finding(chat_id,fingerprint,area,path,side,line,body,created_at) VALUES(2,'fresh','untracked','new-file','new',1,'foreign','now')", []).is_err());
        assert!(conn.execute("INSERT INTO chat_checkout_finding(chat_id,fingerprint,area,path,side,line,body,created_at) VALUES(1,'fresh','invented','new-file','new',1,'bad area','now')", []).is_err());
    }

    /// A v42 database with a row in every table the thinking-level rebuild touches, and
    /// in enough of their dependents to prove the rebuild kept what pointed at them.
    fn seed_v42(path: &Path) {
        let conn = Connection::open(path).unwrap();
        configure(&conn).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").unwrap();
        for (version, name, sql) in MIGRATIONS.iter().filter(|(v, _, _)| *v <= 42) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations VALUES (?1, ?2, 'fixture')",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO project (id, slug, name, created_at, updated_at) VALUES (1,'p','P','t','t');
             INSERT INTO team (id, project_id, slug, name, created_at, updated_at) VALUES (1,1,'t','T','t','t');
             UPDATE project SET team_id = 1 WHERE id = 1;
             INSERT INTO agent (id, team_id, role, name, model, prompt_preset, reasoning, zone, created_at, updated_at)
             VALUES (5,1,'backend','B','glm-4.6','backend','high','crates/**','t','t');
             INSERT INTO run (id, project_id, prompt, created_at, updated_at) VALUES (1,1,'do it','t','t');
             -- One turn that recorded its level, and one from before 042 that could not.
             INSERT INTO node_run (id, run_id, agent_id, role, provider, model, reasoning, tokens_in, created_at, updated_at)
             VALUES (9,1,5,'backend','zai','glm-4.6','high',1234,'t','t'),
                    (10,1,5,'backend','zai','glm-4.6',NULL,7,'t','t');
             INSERT INTO chat (id, project_id, title, workspace_path, provider, model, reasoning, active_node_id, created_at, updated_at)
             VALUES (3,1,'C','/w','zai','glm-4.6','medium',9,'t','t');
             INSERT INTO chat_turn (chat_id, run_id, node_id, request_id) VALUES (3,1,9,'req');
             INSERT INTO event (id, run_id, node_run_id, at, kind, summary) VALUES (50,1,9,'t','step','evidence');
             INSERT INTO chat_push_grant (id, chat_id, request_id, node_id, mode, message, workspace_path,
                 workspace_epoch, branch, origin_url, head_sha, supervisor_pid, supervisor_identity, state, created_at, updated_at)
             VALUES (4,3,'req',9,'single','m','/w',1,'b','u','sha',111,'ident','armed','t','t');
             -- An AUTOINCREMENT high-water mark that must survive untouched.
             INSERT INTO chat_checkout_finding VALUES (77,3,'fp',NULL,'staged','f','new',1,'body','t');
             DELETE FROM chat_checkout_finding WHERE id = 77;",
        )
        .unwrap();
    }

    fn upgrade_thinking_only(path: &Path) -> Db {
        let conn = Connection::open(path).unwrap();
        configure(&conn).unwrap();
        let mut db = Db {
            conn,
            path: path.to_path_buf(),
        };
        let (version, name, sql) = MIGRATIONS
            .iter()
            .find(|(version, _, _)| *version == 43)
            .unwrap();
        db.rebuild(*version, name, sql).unwrap();
        db
    }

    #[test]
    fn observations_upgrade_v43_without_rewriting_or_backfilling_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v43.db");
        seed_v42(&path);
        let db = upgrade_thinking_only(&path);
        let query = "SELECT json_group_array(json_array(id,run_id,node_run_id,at,kind,actor,summary,payload_json,eve_event_id)) FROM event";
        let before = snapshot(db.conn(), query);
        let trigger_query = "SELECT json_group_array(sql) FROM sqlite_master WHERE type='trigger' AND tbl_name='event'";
        let triggers = snapshot(db.conn(), trigger_query);
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), latest_schema());
        assert_eq!(snapshot(db.conn(), query), before);
        assert_eq!(
            snapshot(
                db.conn(),
                "SELECT json_group_array(observation_json) FROM event"
            ),
            "[null]"
        );
        assert!(db
            .conn()
            .execute("UPDATE event SET observation_json='{}' WHERE id=50", [])
            .is_err());
        assert_eq!(
            snapshot(db.conn(), trigger_query),
            triggers,
            "preserve the update guard and existing deletion/cascade behavior"
        );
        assert!(db.conn().execute("INSERT INTO event(run_id,at,kind,observation_json) VALUES(1,'now','step','not json')", []).is_err());
        drop(db);
        assert_eq!(snapshot(Db::open(&path).unwrap().conn(), query), before);
    }

    fn snapshot(conn: &Connection, sql: &str) -> String {
        conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap()
    }

    /// Every schema object, as `type|name|tbl_name` paired with its DDL. A rename leaves
    /// the new name quoted, which is cosmetic, so quotes come out before comparing.
    fn objects(conn: &Connection) -> Vec<(String, String)> {
        let mut q = conn
            .prepare(
                "SELECT type||'|'||name||'|'||tbl_name, COALESCE(sql,'') FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .unwrap();
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?.replace('"', ""),
                ))
            })
            .unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn thinking_levels_widen_the_checks_without_disturbing_a_row_id_or_schema_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v42.db");
        seed_v42(&path);

        let agents = "SELECT json_group_array(json_array(id,team_id,ord,role,name,purpose,provider,model,
            reasoning,zone,prompt_preset,prompt_md,read_only,enabled,rev,created_at,updated_at,context_window))
            FROM (SELECT * FROM agent ORDER BY id)";
        let nodes = "SELECT json_group_array(json_array(id,run_id,agent_id,role,provider,model,status,attempt,
            slice_key,worktree_path,branch,lease_id,session_id,tokens_in,tokens_out,tokens_cache_read,
            tokens_cache_write,turns,blocked_reason,started_at,ended_at,rev,created_at,updated_at,stream_cursor,
            eve_port,eve_token,context_tokens,session_retired_at,session_resetting_at,pushed_at,pr_url,
            merge_requested_at,delivery_claim,delivery_claimed_at,delivery_error,supervisor_pid,task_key,
            push_replaces,pi_pid,reasoning)) FROM (SELECT * FROM node_run ORDER BY id)";
        let chats = "SELECT json_group_array(json_array(id,project_id,title,workspace_path,provider,model,
            reasoning,active_node_id,live_text,supervisor_identity,pi_identity,stop_requested,archived,rev,
            created_at,updated_at,mode)) FROM (SELECT * FROM chat ORDER BY id)";
        let sequences =
            "SELECT json_group_array(json_array(name,seq)) FROM (SELECT * FROM sqlite_sequence ORDER BY name)";

        let (before, before_objects) = {
            let conn = Connection::open(&path).unwrap();
            let taken = [agents, nodes, chats, sequences].map(|q| snapshot(&conn, q));
            // The old CHECK is the thing this migration exists to replace.
            assert!(
                conn.execute("UPDATE agent SET reasoning='xhigh' WHERE id=5", [])
                    .is_err(),
                "fixture must start on the narrow CHECK"
            );
            (taken, objects(&conn))
        };

        // This exact-schema assertion certifies migration 43, not later additive migrations.
        let db = upgrade_thinking_only(&path);
        assert_eq!(db.schema_version().unwrap(), 43);
        let conn = db.conn();

        let after = [agents, nodes, chats, sequences].map(|q| snapshot(conn, q));
        for (label, (before, after)) in ["agent", "node_run", "chat", "sequences"]
            .iter()
            .zip(before.iter().zip(after.iter()))
        {
            assert_eq!(before, after, "{label} rows changed across the rebuild");
        }

        // The whole migration, stated as an assertion: every schema object is the one
        // that was there, and the only text that changed anywhere is the three widened
        // CHECKs. Indexes, triggers and views have to match byte for byte, so one quietly
        // dropped or recreated differently fails here rather than in a demo.
        let widened = |sql: &str| {
            sql.replace(
                "'none','low','medium','high'",
                "'none','minimal','low','medium','high','xhigh','max'",
            )
            .replace(
                "'none', 'low', 'medium', 'high'",
                "'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'",
            )
        };
        let after_objects = objects(conn);
        assert_eq!(
            before_objects.len(),
            after_objects.len(),
            "a schema object was added or lost"
        );
        for (before, after) in before_objects.iter().zip(after_objects.iter()) {
            assert_eq!(before.0, after.0, "schema object identity changed");
            assert_eq!(
                widened(&before.1),
                after.1,
                "{} changed by more than its thinking CHECK",
                before.0
            );
        }
        assert_eq!(
            before_objects
                .iter()
                .filter(|(_, sql)| widened(sql) != *sql)
                .count(),
            3,
            "exactly agent, chat and node_run carry a thinking CHECK"
        );

        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name LIKE 'v_%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            VIEW_COUNT
        );
    }

    #[test]
    fn a_widened_thinking_check_takes_every_level_and_still_refuses_an_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v42.db");
        seed_v42(&path);
        let db = Db::open(&path).unwrap();
        let conn = db.conn();

        // Every new level is storable, in both tables that take one from a person.
        for level in crate::Reasoning::ALL {
            conn.execute("UPDATE agent SET reasoning=?1 WHERE id=5", [level])
                .unwrap_or_else(|e| panic!("agent rejected {level}: {e}"));
            conn.execute("UPDATE chat SET reasoning=?1 WHERE id=3", [level])
                .unwrap_or_else(|e| panic!("chat rejected {level}: {e}"));
        }
        // Aliases are an input spelling, never a stored one, so the column still refuses
        // them - otherwise one level would have two spellings and the CHECK could not say
        // which was real.
        for alias in ["off", "min", "", "ultra"] {
            assert!(
                conn.execute("UPDATE agent SET reasoning=?1 WHERE id=5", [alias])
                    .is_err(),
                "agent.reasoning accepted {alias:?}"
            );
            assert!(
                conn.execute("UPDATE chat SET reasoning=?1 WHERE id=3", [alias])
                    .is_err(),
                "chat.reasoning accepted {alias:?}"
            );
        }

        // A turn's recorded level is still evidence: immutable, and unknown when the turn
        // predates 042 rather than silently becoming the chat's current preference.
        assert!(conn
            .execute("UPDATE node_run SET reasoning='max' WHERE id=9", [])
            .unwrap_err()
            .to_string()
            .contains("immutable"));
        let historical: Option<String> = conn
            .query_row("SELECT reasoning FROM node_run WHERE id=10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(historical, None);

        // What pointed at the rebuilt tables still points at them, and is still enforced.
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM event e JOIN node_run n ON n.id=e.node_run_id
                 JOIN chat_turn t ON t.node_id=n.id JOIN chat c ON c.id=t.chat_id
                 JOIN agent a ON a.id=n.agent_id WHERE e.id=50 AND a.id=5",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1,
            "the rebuild must hand foreign keys back on"
        );
        assert!(conn
            .execute(
                "INSERT INTO chat_turn (chat_id,run_id,node_id,request_id) VALUES (3,1,404,'x')",
                []
            )
            .is_err());
        assert_eq!(
            conn.query_row("PRAGMA foreign_key_check", [], |r| r.get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );

        // The triggers that rode on these tables came back with them.
        conn.execute("UPDATE chat SET archived=1, updated_at='t2' WHERE id=3", [])
            .unwrap();
        assert_eq!(
            snapshot(conn, "SELECT state FROM chat_push_grant WHERE id=4"),
            "expired"
        );
    }

    #[test]
    fn a_rebuild_that_fails_changes_nothing_and_hands_foreign_keys_back() {
        // Turning enforcement off is the dangerous part of a rebuild. A connection left
        // that way accepts orphan rows for as long as the process lives, so both exits
        // from `rebuild` have to restore it - the one where the script fails outright,
        // and the one where it succeeds but leaves something pointing at nothing.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v42.db");
        seed_v42(&path);
        let mut db = Db::open(&path).unwrap();
        let settled = db.schema_version().unwrap();
        let before = objects(db.conn());

        for (sql, expected) in [
            // Drops a parent, then fails before it can put anything back.
            (
                "DROP TABLE node_run; DROP TABLE no_such_table;",
                "no_such_table",
            ),
            // Succeeds as a script, but strands every row that referenced the turn.
            // Which table `foreign_key_check` names first is its business; that it named
            // one, and that the migration stopped, is the contract.
            ("DELETE FROM node_run;", "referencing rows that are gone"),
        ] {
            let error = db.rebuild(999, "deliberate", sql).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert_eq!(
                db.conn()
                    .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1,
                "foreign keys were left off after: {error}"
            );
            assert_eq!(
                db.conn()
                    .query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(db.schema_version().unwrap(), settled);
            assert_eq!(
                objects(db.conn()),
                before,
                "a failed rebuild changed the schema"
            );
            assert_eq!(
                db.conn()
                    .query_row("SELECT COUNT(*) FROM node_run", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                2,
                "a failed rebuild kept its rows"
            );
        }
    }

    #[test]
    fn released_schema_upgrades_without_reusing_chat_development_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("released.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").unwrap();
        for (version, name, sql) in MIGRATIONS.iter().filter(|(v, _, _)| *v <= 17) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations VALUES (?1, ?2, 'fixture')",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO project (slug, name, created_at, updated_at) VALUES ('keep', 'Keep', 'fixture', 'fixture')", []).unwrap();
        drop(conn);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), latest_schema());
        assert_eq!(
            db.conn()
                .query_row("SELECT name FROM project WHERE slug='keep'", [], |r| r
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "Keep"
        );
        db.conn()
            .prepare("SELECT on_default_branch, supervisor_pid FROM run")
            .unwrap();
        db.conn()
            .prepare("SELECT task_key, push_replaces, pi_pid FROM node_run")
            .unwrap();
        db.conn().prepare("SELECT id FROM chat").unwrap();
        db.conn()
            .prepare("SELECT chat_id, after_node_id, state, prompt_sha FROM chat_followup")
            .unwrap();
        db.conn()
            .prepare("SELECT id, state, child_journal FROM chat_delivery")
            .unwrap();
        db.conn()
            .prepare("SELECT project_id, path FROM toolbox_root")
            .unwrap();
        db.conn()
            .prepare("SELECT id, snapshot_json, state, outcome_json FROM toolbox_preview")
            .unwrap();
    }

    #[test]
    fn an_old_chat_development_database_is_refused_without_reinterpreting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("development.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").unwrap();
        for (version, name, sql) in MIGRATIONS.iter().filter(|(v, _, _)| *v <= 13) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations VALUES (?1, ?2, 'fixture')",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        conn.execute_batch(include_str!("migrations/018_chats.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations VALUES (14, 'chats', 'fixture')",
            [],
        )
        .unwrap();
        drop(conn);
        let error = Db::open(&path).unwrap_err().to_string();
        assert!(error.contains("pre-release chat schema"), "{error}");
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            14
        );
        assert!(conn.prepare("SELECT task_key FROM node_run").is_err());
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
    fn a_sessions_context_is_read_again_from_the_evidence_it_left() {
        // Rule 12 once read a step's prompt as `input` alone, which Pi reports beside the
        // cache figures, so every recorded session held a token or two. The events kept
        // the whole usage, and the latest step's prompt is what the session held.
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
        for (version, name, sql) in MIGRATIONS.iter().filter(|(version, _, _)| *version <= 16) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        let at = "'2026-01-01T00:00:00Z'";
        conn.execute_batch(&format!(
            "INSERT INTO project (id, slug, name, created_at, updated_at)
             VALUES (1, 'p', 'P', {at}, {at});
             INSERT INTO run (id, project_id, prompt, created_at, updated_at)
             VALUES (1, 1, 'build it', {at}, {at});
             INSERT INTO node_run
                    (id, run_id, role, provider, model, context_tokens, created_at, updated_at)
             VALUES (1, 1, 'frontend', 'claude', 'sonnet', 1, {at}, {at}),
                    (2, 1, 'backend', 'claude', 'sonnet', 7, {at}, {at});
             INSERT INTO event (run_id, node_run_id, at, kind, payload_json) VALUES
               (1, 1, {at}, 'cost', '{{\"message\":{{\"usage\":{{\"input\":1,\"output\":90,
                  \"cacheRead\":60000,\"cacheWrite\":500,\"totalTokens\":60591}}}}}}'),
               (1, 1, {at}, 'cost', '{{\"message\":{{\"usage\":{{\"input\":1,\"output\":319,
                  \"cacheRead\":68881,\"cacheWrite\":257,\"totalTokens\":69458}}}}}}'),
               (1, 1, {at}, 'note', '{{\"usage\":{{\"input\":0,\"output\":0,
                  \"totalTokens\":0}}}}'),
               (1, 2, {at}, 'note', '{{\"summary\":\"no usage recorded\"}}');"
        ))
        .unwrap();
        drop(conn);

        let db = Db::open(&path).unwrap();
        let context = |id: i64| -> Option<i64> {
            db.conn()
                .query_row(
                    "SELECT context_tokens FROM node_run WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        // The latest step's whole prompt, cached or not.
        assert_eq!(context(1), Some(69_139));
        // A turn that left no usage keeps what it had rather than a guess.
        assert_eq!(context(2), Some(7));
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
