//! Scheduling into an exact chat, over the real socket. No installed tools, no model
//! turn: the clock is not run here, so nothing in this binary can start a Pi process.
#![cfg(unix)]

use ai_team_core::{
    CredentialStore, NewChat, NewProject, NewRepo, Provider, Reasoning, ScheduleOutcome, Store,
    DEFAULT_MACHINE_PROFILE,
};
use ai_team_ui::{ServeOptions, Server};
use std::{os::unix::fs::PermissionsExt, time::Duration};

struct AbortServer(tokio::task::AbortHandle);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn call(
    address: std::net::SocketAddr,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let payload = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{}: {token}\r\nContent-Type: \
         application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        ai_team_ui::TOKEN_HEADER,
        payload.len()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let status: u16 = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let payload = response.split_once("\r\n\r\n").map_or("", |(_, rest)| rest);
    (
        status,
        serde_json::from_str(payload).unwrap_or(serde_json::Value::String(payload.into())),
    )
}

/// Isolated: its own home, its own credentials, its own PATH. The one test in this binary
/// owns the process environment, exactly as the other server fixtures do.
async fn fixture() -> (tempfile::TempDir, std::path::PathBuf, i64, Server) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    for key in ["HOME", "XDG_CONFIG_HOME", "AI_TEAM_HOME"] {
        std::env::set_var(key, &root);
    }
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", root.display()));
    // A catalogue, so model policy can be resolved without the operator's own Pi.
    let pi = root.join("pi");
    std::fs::write(&pi, "#!/bin/sh\ncase \"$*\" in *--list-models*) printf '%s\\n' 'provider model context max-out thinking images' 'ailocal fixture-model 200k 32k yes yes' ;; *) exit 99 ;; esac\n").unwrap();
    std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profile = ai_team_core::machine_profile_path().unwrap();
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(
        &profile,
        DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
    )
    .unwrap();

    let checkout = root.join("widget");
    std::fs::create_dir_all(&checkout).unwrap();
    let db = root.join("team.db");
    let mut store = Store::init(&db).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .attach_repo(
            project.id,
            NewRepo {
                main_path: Some(checkout.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: checkout,
            provider: Provider::Local,
            model: "fixture-model".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    drop(store);

    let server = Server::bind(ServeOptions {
        db_path: Some(db.clone()),
        credentials: CredentialStore::isolated(),
        ..Default::default()
    })
    .await
    .unwrap();
    (dir, db, chat.id, server)
}

#[tokio::test]
async fn scheduled_work_is_made_for_one_chat_and_reports_back_to_it() {
    let (_dir, db, chat, server) = fixture().await;
    let address = server.addr();
    let token = server.token().to_owned();
    let task = tokio::spawn(server.serve());
    let _abort = AbortServer(task.abort_handle());

    // A schedule needs the chat, and the prompt it will actually send.
    let (status, made) = call(
        address,
        &token,
        "POST",
        "/api/reminders",
        Some(
            r#"{"kind":"scheduled_run","chat":1,"title":"nightly sweep",
                "prompt":"tidy the flaky tests","due_at":"2020-01-01T00:00:00Z"}"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "{made}");
    assert_eq!(made["chat_id"], chat, "{made}");
    assert_eq!(made["kind"], "scheduled_run");

    // A reminder or an idea has no chat to run in, and saying so is better than making
    // one that silently never fires.
    let (status, refused) = call(
        address,
        &token,
        "POST",
        "/api/reminders",
        Some(r#"{"kind":"idea","chat":1,"title":"what if"}"#),
    )
    .await;
    assert_eq!(status, 400, "{refused}");

    // And a schedule with nothing to say is refused rather than sending an empty prompt
    // to a model at two in the morning.
    let (status, empty) = call(
        address,
        &token,
        "POST",
        "/api/reminders",
        Some(
            r#"{"kind":"scheduled_run","chat":1,"title":"nightly",
                "due_at":"2020-01-01T00:00:00Z"}"#,
        ),
    )
    .await;
    assert_eq!(status, 400, "{empty}");

    let (status, listed) = call(address, &token, "GET", "/api/chat-schedules", None).await;
    assert_eq!(status, 200, "{listed}");
    let schedule = &listed.as_array().unwrap()[0];
    assert_eq!(schedule["chat_id"], chat);
    assert_eq!(schedule["chat_title"], "New chat");
    assert_eq!(schedule["project_slug"], "widget");
    assert_eq!(schedule["prompt"], "tidy the flaky tests");
    // The model context is the schedule's own, recorded now rather than read later.
    assert_eq!(schedule["model"], "fixture-model");
    assert_eq!(schedule["mode"], "single");
    assert!(schedule["occurrences"].as_array().unwrap().is_empty());

    // The clock, without its supervisor: claiming and dispatching write the result, and
    // the turn itself is somebody else's to drive.
    let mut store = Store::open(&db).unwrap();
    let claimed = ai_team_core::claim_due(&mut store, &ai_team_core::now()).unwrap();
    let dispatched = ai_team_core::dispatch_chat_schedule(&db, claimed[0].chat.as_ref().unwrap())
        .await
        .unwrap();
    assert!(dispatched.started(), "{}", dispatched.detail);
    drop(store);

    let (_, after) = call(address, &token, "GET", "/api/chat-schedules", None).await;
    let occurrence = &after.as_array().unwrap()[0]["occurrences"][0];
    assert_eq!(occurrence["outcome"], "started");
    assert_eq!(occurrence["run_id"], dispatched.run_id.unwrap());
    assert_eq!(occurrence["node_id"], dispatched.node_id.unwrap());
    // The turn is in that exact chat, with the prompt that was scheduled.
    let (_, detail) = call(address, &token, "GET", "/api/chats/1", None).await;
    let turns = detail["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1, "{detail}");
    assert_eq!(turns[0]["run"]["prompt"], "tidy the flaky tests");
    assert_eq!(detail["state"], "running");

    // A second schedule, due while that turn is still working, is skipped rather than
    // queued - and says so where the chat can be opened from.
    let (status, second) = call(
        address,
        &token,
        "POST",
        "/api/reminders",
        Some(
            r#"{"kind":"scheduled_run","chat":1,"title":"second sweep",
                "prompt":"and again","due_at":"2020-01-02T00:00:00Z"}"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "{second}");
    let mut store = Store::open(&db).unwrap();
    let claimed = ai_team_core::claim_due(&mut store, &ai_team_core::now()).unwrap();
    let skipped = ai_team_core::dispatch_chat_schedule(&db, claimed[0].chat.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(skipped.outcome, ScheduleOutcome::Busy, "{}", skipped.detail);
    drop(store);

    let (_, notices) = call(address, &token, "GET", "/api/notifications", None).await;
    let notice = notices
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "follow_up")
        .expect("a schedule that did nothing is still attention");
    assert_eq!(notice["chat_id"], chat, "{notice}");
    assert!(notice["title"].as_str().unwrap().contains("did not start"));

    // Nothing ran twice, and nothing was queued behind the running turn.
    let (_, unchanged) = call(address, &token, "GET", "/api/chats/1", None).await;
    assert_eq!(unchanged["turns"].as_array().unwrap().len(), 1);
}
