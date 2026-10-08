use super::{Harness, ServeOptions};
use ai_team_core::{Host, Method, Store, UpdateInstallation, UpdateManager};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn cli(path: &Path, version: &str) {
    fs::write(path, format!("#!/bin/sh\nprintf 'ait {version}\\n'\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn app(path: &Path, version: &str) {
    fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    fs::write(
        path.join("Contents/MacOS/ai-team"),
        "offline desktop fixture",
    )
    .unwrap();
    fs::write(path.join("Contents/Info.plist"), format!("<plist><dict><key>CFBundleIdentifier</key><string>dev.zottiben.ai-team</string><key>CFBundleExecutable</key><string>ai-team</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>")).unwrap();
}

fn fixture() -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for folder in ["home", "bin", "state", "releases", "payload", "config"] {
        fs::create_dir(root.join(folder)).unwrap();
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
        .args(["ait", "ai-team.app"])
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
    let digest = hash.arg(&archive).output().unwrap();
    assert!(digest.status.success());
    let digest = String::from_utf8(digest.stdout).unwrap();
    fs::write(
        root.join("releases/checksums.txt"),
        format!("{}  {asset}\n", digest.split_whitespace().next().unwrap()),
    )
    .unwrap();
    let db = root.join("state/team.db");
    let store = Store::init(&db).unwrap();
    let installation = UpdateInstallation {
        host: Host::Desktop,
        cli: Some(root.join("bin/ait")),
        desktop: Some(root.join("ai-team.app")),
        home: root.join("home"),
        data_dir: root.join("state"),
        database: Some(db.clone()),
        config_dir: Some(root.join("config")),
        method: Method::Release,
        blocked: None,
    };
    let updates = UpdateManager::isolated(installation, &root).unwrap();
    let h = Harness::bound(
        ServeOptions {
            store: Some(store),
            db_path: Some(db),
            updates,
            host: Host::Desktop,
            ..Default::default()
        },
        None,
    );
    (dir, h)
}

#[test]
fn update_http_is_authenticated_read_only_until_exact_approval_and_fences_old_settings_writes() {
    let (dir, mut h) = fixture();
    assert_eq!(h.get_anonymous("/api/update").status, 401);
    let token = std::mem::replace(&mut h.token, "wrong".into());
    assert_eq!(
        h.post("/api/update", r#"{"version":"0.7.6","approval":"unknown"}"#)
            .status,
        401
    );
    h.token = token;
    let checked = h.get("/api/update");
    assert_eq!(checked.status, 200, "{}", checked.body);
    let checked = checked.json();
    assert_eq!(checked["state"], "idle");
    assert_eq!(checked["targets"].as_array().unwrap().len(), 2);
    assert_eq!(checked["can_update"], true);
    assert!(!dir.path().join("state/updates").exists());
    assert_eq!(h.get("/api/update?destination=elsewhere").status, 400);
    assert_eq!(
        h.post("/api/update", r#"{"version":"0.7.6","approval":"stale"}"#)
            .status,
        400
    );
    let approval = serde_json::json!({"version":"0.7.6", "approval":checked["approval"]});
    let mut injected = approval.clone();
    injected["destination"] = "elsewhere".into();
    assert_eq!(h.post("/api/update", &injected.to_string()).status, 422);
    assert_eq!(
        h.post("/api/update/inspect", r#"{"path":"elsewhere"}"#)
            .status,
        422
    );
    assert!(!dir.path().join("state/updates").exists());
    let installed = h.post("/api/update", &approval.to_string());
    assert_eq!(installed.status, 200, "{}", installed.body);
    assert_eq!(installed.json()["version"], "0.7.6");
    assert_eq!(h.get("/api/update").json()["state"], "restart");
    assert_eq!(h.post("/api/update", &approval.to_string()).status, 400);
    // An isolated credential store ensures that even a regression cannot touch Keychain.
    let refused = h.post(
        "/api/settings/token",
        r#"{"source":"clickup","token":"fixture-only"}"#,
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(refused.body.contains("restart"));
}

#[test]
fn an_update_fence_also_blocks_non_database_mutations_without_blocking_checks() {
    let (dir, h) = fixture();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join("state/.app-update.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let refused = h.post(
        "/api/settings/token",
        r#"{"source":"clickup","token":"fixture-only"}"#,
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(refused.body.contains("updating"));
    assert_eq!(h.get("/api/update").json()["state"], "updating");
    drop(lock);
    assert_eq!(
        h.post(
            "/api/settings/token",
            r#"{"source":"clickup","token":"fixture-only"}"#
        )
        .status,
        200
    );
}
