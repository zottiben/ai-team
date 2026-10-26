//! A library server must not discover or migrate the operator's database.

use std::process::Command;

use ai_team_ui::{ServeOptions, Server};

#[tokio::test]
async fn a_newer_database_reports_its_schema_error_not_initialization_advice() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("team.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL);",
    )
    .unwrap();
    let future = ai_team_core::latest_schema() + 1;
    conn.execute(
        "INSERT INTO schema_migrations VALUES(?1, 'future', 'fixture')",
        [future],
    )
    .unwrap();
    drop(conn);
    let server = Server::bind(ServeOptions {
        db_path: Some(db.clone()),
        token: Some("isolated-token".into()),
        credentials: ai_team_core::CredentialStore::isolated(),
        ..Default::default()
    })
    .await
    .unwrap();
    let address = server.addr();
    let task = tokio::spawn(server.serve());
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(b"GET /api/projects HTTP/1.1\r\nHost: localhost\r\nx-ai-team-token: isolated-token\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    task.abort();
    assert!(
        response.contains(&format!("database schema v{future}")),
        "{response}"
    );
    assert!(!response.contains("run `ait init`"), "{response}");
    let conn = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        future
    );
}

#[test]
fn an_unscoped_server_leaves_the_machine_database_untouched() {
    const ROOT: &str = "AI_TEAM_SERVER_ISOLATION_ROOT";
    if let Ok(root) = std::env::var(ROOT) {
        let root = std::path::PathBuf::from(root);
        let db = ai_team_core::default_db_path().unwrap();
        assert!(db.starts_with(&root));
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        // Deliberately old but valid application data. Opening it through Store would
        // migrate it, just as a new development test migrated the installed app's data.
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL);
             CREATE TABLE sentinel(value TEXT NOT NULL);
             INSERT INTO sentinel VALUES('keep my chat');",
        )
        .unwrap();
        drop(conn);
        let before = std::fs::read(&db).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                token: Some("isolated-token".into()),
                credentials: ai_team_core::CredentialStore::isolated(),
                ..Default::default()
            }))
            .unwrap();
        assert_eq!(server.token(), "isolated-token");
        drop(server);
        drop(runtime);
        assert!(
            std::fs::read(&db).unwrap() == before,
            "a server with no database scope migrated the machine database"
        );
        assert!(
            !db.parent().unwrap().join(".app-update.lock").exists(),
            "unscoped startup reached the machine's update fence"
        );
        return;
    }

    // Integration-test libraries lack cfg(test). Prove the production default in a
    // subprocess with a private HOME instead of ever pointing this test at real data.
    for override_data in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "an_unscoped_server_leaves_the_machine_database_untouched",
                "--nocapture",
            ])
            .env(ROOT, root.path())
            .env("HOME", root.path())
            .env("USERPROFILE", root.path())
            .env_remove("AI_TEAM_HOME")
            .env("XDG_CONFIG_HOME", root.path().join("config"));
        if override_data {
            child.env("AI_TEAM_HOME", root.path());
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
