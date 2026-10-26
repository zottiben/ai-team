use super::Db;
use crate::{Dispatch, DispatchClaim, NewProject, PressRequest, Store};
use rusqlite::{params, Connection};
use std::path::PathBuf;

fn legacy() -> (tempfile::TempDir, PathBuf, i64) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("team.db");
    let mut store = Store::init(&path).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Migration fixture".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    drop(store);
    let conn = Connection::open(&path).unwrap();
    remove_post_50_fixture_schema(&conn);
    conn.execute_batch(include_str!("terminal_legacy.sql"))
        .unwrap();
    conn.execute(
        "INSERT INTO terminal_pad VALUES(7,?1,2,'Test','printf fixture','created','edited')",
        [project],
    )
    .unwrap();
    conn.execute("INSERT INTO terminal_command VALUES(11,?1,'legacy-finished','/fixture','Test','printf fixture',41,'start','end',0,123,9,0)", [project]).unwrap();
    conn.execute("INSERT INTO terminal_command VALUES(12,?1,'legacy-unfinished','/fixture','Test','printf fixture',42,'later',NULL,NULL,NULL,2,1)", [project]).unwrap();
    conn.execute_batch("CREATE INDEX fixture_terminal_index ON terminal_command(label);
        CREATE TABLE fixture_audit (token TEXT);
        CREATE TRIGGER fixture_terminal_trigger AFTER UPDATE ON terminal_command BEGIN INSERT INTO fixture_audit VALUES(new.token); END;").unwrap();
    (dir, path, project)
}

// These terminal fixtures deliberately rewind a fresh schema to development v49.
// Rewind the additive schema too, not just its ledger: otherwise reopening attempts
// to add an already-present column. No queued rows are seeded by these fixtures.
fn remove_post_50_fixture_schema(conn: &Connection) {
    conn.execute_batch(
        "DROP TRIGGER chat_followup_model_immutable;
        ALTER TABLE chat_followup DROP COLUMN model_settings_json;",
    )
    .unwrap();
}

#[test]
fn legacy_terminal_rows_survive_and_new_presses_keep_exact_identity() {
    let (_dir, path, project) = legacy();
    let mut store = Store::open(&path).unwrap();
    let pads = store.terminal_pads(project).unwrap();
    assert_eq!((pads[0].id, pads[0].position, pads[0].revision), (7, 2, 1));
    assert_eq!(
        (pads[0].label.as_str(), pads[0].command.as_str()),
        ("Test", "printf fixture")
    );
    let rows = store.terminal_command_history(project, "/fixture").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (
            rows[0].id,
            rows[0].session_id,
            rows[0].finished_at.as_deref()
        ),
        (12, Some(42), None)
    );
    assert_eq!(
        (
            rows[1].token.as_str(),
            rows[1].exit_code,
            rows[1].duration_ms,
            rows[1].lines,
            rows[1].lines_exact
        ),
        ("legacy-finished", Some(0), Some(123), Some(9), false)
    );
    for row in &rows {
        assert_eq!(row.dispatch, Dispatch::Started);
        assert_eq!(
            (row.pad_id, row.revision, row.claimed_at.as_str()),
            (0, 0, "")
        );
        assert!(row.request_id.starts_with("legacy:"));
        assert!(store
            .claim_terminal_dispatch(
                project,
                &PressRequest {
                    request_id: &row.request_id,
                    worktree: "/fixture",
                    pad: 7,
                    revision: 1,
                    confirmed: "printf fixture"
                }
            )
            .is_err());
    }
    let request = PressRequest {
        request_id: "new-request-after-upgrade",
        worktree: "/fixture",
        pad: 7,
        revision: 1,
        confirmed: "printf fixture",
    };
    assert!(matches!(
        store.claim_terminal_dispatch(project, &request).unwrap(),
        DispatchClaim::Fresh(_)
    ));
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(matches!(
        store.claim_terminal_dispatch(project, &request).unwrap(),
        DispatchClaim::Replay(_)
    ));
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM terminal_command", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM fixture_audit", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute("UPDATE terminal_command SET lines=10 WHERE id=11", [])
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT token FROM fixture_audit", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "legacy-finished"
    );
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('fixture_terminal_index','event_visible_node')", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(store.schema_version().unwrap(), super::latest_schema());
}

#[test]
fn legacy_and_fresh_terminal_schemas_match_exactly() {
    let (_dir, path, _) = legacy();
    Store::open(&path).unwrap();
    let conn = Connection::open(path).unwrap();
    let fresh = Db::memory().unwrap();
    for table in [
        "terminal_pad",
        "terminal_command",
        "terminal_pad_project",
        "terminal_command_history",
        "event_visible_node",
    ] {
        let sql = |conn: &Connection| {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1",
                [table],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        };
        assert_eq!(sql(&conn), sql(fresh.conn()));
    }
}

#[test]
fn development_receipts_are_preserved_byte_for_byte_by_the_forward_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("team.db");
    let mut store = Store::init(&path).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Migration fixture".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let pad = store
        .add_terminal_pad(project, "Test", "printf fixture")
        .unwrap();
    let request = PressRequest {
        request_id: "existing-development-request",
        worktree: "/fixture",
        pad: pad.id,
        revision: pad.revision,
        confirmed: &pad.command,
    };
    store.claim_terminal_dispatch(project, &request).unwrap();
    let before = store.terminal_command_history(project, "/fixture").unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    remove_post_50_fixture_schema(&conn);
    conn.execute("DELETE FROM schema_migrations WHERE version>49", [])
        .unwrap();
    drop(conn);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.terminal_command_history(project, "/fixture").unwrap(),
        before
    );
    assert!(matches!(
        store.claim_terminal_dispatch(project, &request).unwrap(),
        DispatchClaim::Replay(_)
    ));
    assert_eq!(store.schema_version().unwrap(), super::latest_schema());
}

#[test]
fn unrecognised_terminal_layout_fails_closed_without_changing_data_or_version() {
    let (_dir, path, project) = legacy();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE terminal_pad ADD COLUMN unexpected TEXT NOT NULL DEFAULT 'keep me'",
    )
    .unwrap();
    let before: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='terminal_pad'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let error = Store::open(&path).unwrap_err().to_string();
    assert!(error.contains("terminal_pad"), "{error}");
    assert_eq!(
        conn.query_row("SELECT max(version) FROM schema_migrations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        49
    );
    assert_eq!(
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE name='terminal_pad'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        before
    );
    assert_eq!(
        conn.query_row(
            "SELECT unexpected FROM terminal_pad WHERE project_id=?1",
            params![project],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "keep me"
    );
}

#[test]
fn a_failed_foreign_key_check_rolls_back_both_tables_and_restores_enforcement() {
    let (_dir, path, project) = legacy();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute("DELETE FROM project WHERE id=?1", [project])
        .unwrap();
    let before: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='terminal_command'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    super::configure(&conn).unwrap();
    let mut db = Db { conn, path };
    let error = db.migrate().unwrap_err().to_string();
    assert!(error.contains("referencing rows that are gone"), "{error}");
    assert_eq!(db.schema_version().unwrap(), 49);
    assert_eq!(
        db.conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.conn
            .query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='terminal_command'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        before
    );
    assert_eq!(
        db.conn
            .query_row("SELECT count(*) FROM terminal_command", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(db.conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('terminal_pad_next','terminal_command_next','event_visible_node')", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}
