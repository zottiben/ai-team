//! Setup uses chat prerequisites, without probing standalone planning or toolbox state.
#![cfg(unix)]

use ai_team_core::{CredentialStore, NewProject, NewRepo, Store, DEFAULT_MACHINE_PROFILE};
use ai_team_ui::{ServeOptions, Server};
use std::{os::unix::fs::PermissionsExt, time::Duration};

struct AbortServer(tokio::task::AbortHandle);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn report(address: std::net::SocketAddr, token: &str) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("GET /api/doctor HTTP/1.1\r\nHost: localhost\r\n{}: {token}\r\nConnection: close\r\n\r\n", ai_team_ui::TOKEN_HEADER).as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

async fn fixture() -> (tempfile::TempDir, ai_team_core::Known, Server) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "AI_TEAM_HOME",
        "READINESS_FIXTURE",
    ] {
        std::env::set_var(key, &root);
    }
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", root.display()));
    for tool in [
        "aip",
        "awt",
        "ai-toolbox",
        "file-sql",
        "claude",
        "codex",
        "security",
    ] {
        let path = root.join(tool);
        std::fs::write(&path, "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> \"$READINESS_FIXTURE/probes\"\nexit 99\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let pi = root.join("pi");
    std::fs::write(&pi, "#!/bin/sh\ncase \"$*\" in auth\\ check*--no-refresh*) printf '%s\\n' '{\"provider\":\"openai-codex\",\"status\":\"ready\",\"authType\":\"oauth\"}' ;; *--list-models*) printf '%s\\n' 'provider model context max-out thinking images' 'openai-codex fixture-model 200k 32k yes yes' ;; *) exit 99 ;; esac\n").unwrap();
    std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let profile = ai_team_core::machine_profile_path().unwrap();
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(
        &profile,
        DEFAULT_MACHINE_PROFILE.replace("openai = false", "openai = true"),
    )
    .unwrap();
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
                main_path: Some(root.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())
        .unwrap();
    let known = ai_team_core::Known::of(&store);
    drop(store);
    let server = Server::bind(ServeOptions {
        db_path: Some(db),
        credentials: CredentialStore::isolated(),
        ..Default::default()
    })
    .await
    .unwrap();
    (dir, known, server)
}

// The sole test owns the environment in this test binary. No installed tools or auth.
#[tokio::test]
async fn desktop_setup_needs_no_standalone_planner_toolbox_or_worktree_install() {
    let (dir, known, server) = fixture().await;
    let root = dir.path();
    let profile = ai_team_core::machine_profile_path().unwrap();
    let address = server.addr();
    let token = server.token().to_owned();
    let task = tokio::spawn(server.serve());
    let _abort = AbortServer(task.abort_handle());

    let ready = report(address, &token).await;
    let checks = ready["checks"].as_array().unwrap();
    assert!(
        !checks.iter().any(|c| matches!(
            c["id"].as_str(),
            Some("aip" | "ai_toolbox" | "planning_skill")
        )),
        "{ready}"
    );
    assert_eq!(ready["can_run"], true, "{ready}");
    assert_eq!(ready["needs_setup"], false, "{ready}");
    assert_ne!(ready["severity"], "blocking", "{ready}");
    for id in ["planning", "toolbox"] {
        assert!(
            checks
                .iter()
                .any(|c| c["id"] == id && c["severity"] == "fine" && c["fix"]["by"] == "none"),
            "{ready}"
        );
    }
    let seats = checks
        .iter()
        .find(|c| c["id"] == "project.widget.seats")
        .unwrap();
    assert_eq!(seats["severity"], "degraded", "{seats}");
    assert!(seats["detail"]
        .as_str()
        .unwrap()
        .starts_with("Optional team:"));
    let awt = checks.iter().find(|c| c["id"] == "awt").unwrap();
    assert_eq!(awt["severity"], "degraded");
    assert!(awt["detail"].as_str().unwrap().contains("team"));
    let probes = std::fs::read_to_string(root.join("probes")).unwrap_or_default();
    assert!(
        !probes
            .lines()
            .any(|line| line.contains("/aip ") || line.contains("/ai-toolbox ")),
        "{probes}"
    );
    assert!(std::fs::read_to_string(profile)
        .unwrap()
        .contains("local = false"));

    // Legacy CLI diagnostics still name their real standalone dependency.
    let legacy =
        ai_team_core::readiness_report_with(Some(&known), &CredentialStore::isolated()).await;
    assert!(legacy
        .checks
        .iter()
        .any(|c| c.id == "aip" && c.severity == ai_team_core::Severity::Blocking));
    assert!(legacy
        .checks
        .iter()
        .any(|c| c.id == "project.widget.seats" && c.severity == ai_team_core::Severity::Blocking));

    // Missing Pi is a real chat prerequisite, even with a subscription logged in.
    std::fs::remove_file(root.join("pi")).unwrap();
    let blocked = report(address, &token).await;
    assert!(
        blocked["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "pi" && c["severity"] == "blocking"),
        "{blocked}"
    );
    assert_eq!(blocked["can_run"], false);
    assert_eq!(blocked["needs_setup"], true);
}
