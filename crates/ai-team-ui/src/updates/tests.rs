use super::*;
use ai_team_core::{Host, Method, Store, UpdateInstallation, UpdateManager};
use axum::{middleware::from_fn_with_state, Router};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
};

fn cli(path: &Path, version: &str) {
    fs::write(path, format!("#!/bin/sh\nprintf 'ait {version}\\n'\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn app(path: &Path, version: &str) {
    fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    fs::write(path.join("Contents/MacOS/ai-team"), "offline app fixture").unwrap();
    fs::write(path.join("Contents/Info.plist"), format!("<plist><dict><key>CFBundleIdentifier</key><string>dev.zottiben.ai-team</string><key>CFBundleExecutable</key><string>ai-team</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>")).unwrap();
}

fn fixture() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for name in ["bin", "state", "home", "payload", "releases"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    cli(&root.join("bin/ait"), "0.7.4");
    app(&root.join("ai-team.app"), "0.7.4");
    cli(&root.join("payload/ait"), "0.7.6");
    app(&root.join("payload/ai-team.app"), "0.7.6");
    fs::write(root.join("state/install-method"), "release\n").unwrap();
    fs::write(root.join("releases/latest"), "0.7.6").unwrap();
    let asset = "ai-team-v0.7.6-macos-universal.tar.gz";
    let archive = root.join("releases").join(asset);
    assert!(Command::new("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(root.join("payload"))
        .arg(".")
        .status()
        .unwrap()
        .success());
    let mut hash = if cfg!(target_os = "macos") {
        let mut command = Command::new("shasum");
        command.args(["-a", "256"]);
        command
    } else {
        Command::new("sha256sum")
    };
    let hash = hash.arg(&archive).output().unwrap();
    assert!(hash.status.success());
    let digest = String::from_utf8(hash.stdout).unwrap();
    fs::write(
        root.join("releases/checksums.txt"),
        format!("{}  {asset}\n", digest.split_whitespace().next().unwrap()),
    )
    .unwrap();
    let db = root.join("state/team.db");
    let store = Store::init(&db).unwrap();
    let manager = UpdateManager::isolated(
        UpdateInstallation {
            host: Host::Desktop,
            cli: Some(root.join("bin/ait")),
            desktop: Some(root.join("ai-team.app")),
            home: root.join("home"),
            data_dir: root.join("state"),
            database: Some(db.clone()),
            config_dir: None,
            method: Method::Release,
            blocked: None,
        },
        &root,
    )
    .unwrap();
    (
        dir,
        AppState::for_server("fixture-token", manager)
            .watching(Some(db))
            .with_store(store),
    )
}

fn request(
    address: SocketAddr,
    method: &str,
    token: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let body = body.to_string();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    write!(stream, "{method} /api/update HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nx-ai-team-token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_update_closes_only_the_exact_reviewed_sessions() {
    let (dir, state) = fixture();
    let terminals = state.terminals().clone();
    let id = terminals.open_program(dir.path(), "/bin/cat", &[]).unwrap();
    terminals
        .protect_update(id, state.updates().admit().unwrap())
        .unwrap();
    let api = crate::api::routes()
        .route_layer(from_fn_with_state(state.clone(), super::admit))
        .route_layer(from_fn_with_state(
            state.clone(),
            crate::auth::require_token,
        ));
    let router = Router::new().nest("/api", api).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (code, checked) = request(address, "GET", "fixture-token", &serde_json::json!({}));
    assert_eq!(code, 200);
    assert_eq!(checked["terminals"]["sessions"][0]["id"], id);
    let mut approval = serde_json::json!({"version":"0.7.6", "approval":checked["approval"], "close_terminals":checked["terminals"]["approval"]});
    assert_eq!(request(address, "POST", "wrong-token", &approval).0, 401);
    let exact = approval["close_terminals"].take();
    let refused = request(address, "POST", "fixture-token", &approval);
    assert_eq!(refused.0, 400);
    assert!(refused.1.to_string().contains("Close sessions and update"));
    approval["close_terminals"] = "unreviewed sessions".into();
    assert_eq!(request(address, "POST", "fixture-token", &approval).0, 400);
    assert!(!terminals.read(id, 0).unwrap().done);
    assert!(!dir.path().join("state/updates").exists());
    approval["close_terminals"] = exact;
    let (code, installed) = request(address, "POST", "fixture-token", &approval);
    assert_eq!(code, 200, "{installed}");
    assert_eq!(installed["version"], "0.7.6");
    assert!(terminals.read(id, 0).unwrap().done);
    assert!(terminals.review_update().sessions.is_empty());
    server.abort();
}
