//! The authenticated direct send path is the only thing that mints push authority.
//!
//! Over the real socket, because what breaks here is the wiring: which handler reads the
//! person's own words, what it does with a team chat, and what it writes down. This
//! binary owns its environment, like the other server fixtures. No model turn succeeds
//! here and no remote is published: the grant row is the evidence.
#![cfg(unix)]

use ai_team_core::{
    ChatMode, CredentialStore, NewChat, NewProject, NewRepo, Provider, Reasoning, Store,
    DEFAULT_MACHINE_PROFILE,
};
use ai_team_ui::{ServeOptions, Server};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

const ASKED: &str = "commit and push and ill open the PR";

/// One checkout, one turn at a time. The fake Pi fails immediately, so this is short.
async fn idle(db: &Path, chat: i64) {
    for _ in 0..600 {
        if Store::open(db)
            .unwrap()
            .chat(chat)
            .unwrap()
            .active_node_id
            .is_none()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("chat {chat} never settled");
}

struct AbortServer(tokio::task::AbortHandle);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn call(
    address: std::net::SocketAddr,
    token: &str,
    path: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\n{}: {token}\r\nContent-Type: \
         application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        ai_team_ui::TOKEN_HEADER,
        body.len()
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(20),
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

/// This binary's own environment, checkout and database. Nothing here is the operator's.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    for key in ["HOME", "XDG_CONFIG_HOME", "AI_TEAM_HOME"] {
        std::env::set_var(key, &root);
    }
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", root.display()));
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    // A catalogue only. Every other invocation fails, so no turn reaches a model.
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

    let repo = root.join("repo");
    let remote = root.join("remote.git");
    std::fs::create_dir(&repo).unwrap();
    git(&root, &["init", "-q", "-b", "work", repo.to_str().unwrap()]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@invalid"]);
    std::fs::write(repo.join("README.md"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    git(
        &root,
        &[
            "init",
            "--bare",
            "-q",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "work:main"]);
    (dir, repo, remote)
}

/// Three chats on one checkout, so each message is read in the mode it was sent in.
fn chats(db: &Path, repo: &Path) -> (i64, i64, i64) {
    let mut store = Store::init(db).unwrap();
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
                main_path: Some(repo.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut chat = |mode| {
        store
            .create_chat_in_mode(
                NewChat {
                    project_id: project.id,
                    workspace: repo.to_path_buf(),
                    provider: Provider::Local,
                    model: "fixture-model".into(),
                    reasoning: Reasoning::High,
                },
                mode,
            )
            .unwrap()
            .id
    };
    (
        chat(ChatMode::Single),
        chat(ChatMode::Single),
        chat(ChatMode::Team),
    )
}

#[tokio::test]
async fn only_a_direct_human_message_mints_authority_and_a_team_push_starts_no_planning() {
    let (dir, repo, remote) = fixture();
    let db = dir.path().canonicalize().unwrap().join("team.db");
    let (solo, vague, team) = chats(&db, &repo);

    let server = Server::bind(ServeOptions {
        db_path: Some(db.clone()),
        credentials: CredentialStore::isolated(),
        ..Default::default()
    })
    .await
    .unwrap();
    let address = server.addr();
    let token = server.token().to_string();
    let _abort = AbortServer(tokio::spawn(server.serve()).abort_handle());

    let send = |chat: i64, message: &str, request: &str| {
        let body =
            serde_json::json!({"message": message, "request_id": request, "workspace_epoch": 0})
                .to_string();
        let token = token.clone();
        async move {
            call(
                address,
                &token,
                &format!("/api/chats/{chat}/messages"),
                &body,
            )
            .await
        }
    };

    // The words the requirement names, sent the way a person sends them.
    let (status, receipt) = send(solo, ASKED, "ask-1").await;
    assert_eq!(status, 200, "{receipt}");
    let store = Store::open(&db).unwrap();
    let grant = store
        .chat_push_grant_for_request(solo, "ask-1")
        .unwrap()
        .expect("the direct send minted what the person asked for");
    assert_eq!(grant.branch, "work");
    assert!(grant.allow_commit);
    assert_eq!(grant.node_id, receipt["node_id"].as_i64());
    drop(store);
    idle(&db, solo).await;

    // Mentioning publishing issues nothing, and the chat is told so in its own thread.
    let (status, answered) = send(vague, "did you push that?", "ask-2").await;
    assert_eq!(status, 200, "{answered}");
    let store = Store::open(&db).unwrap();
    assert!(store
        .chat_push_grant_for_request(vague, "ask-2")
        .unwrap()
        .is_none());
    let summaries: Vec<String> = store
        .chat_events(vague, 0, 500)
        .unwrap()
        .into_iter()
        .map(|event| event.summary)
        .collect();
    assert!(
        summaries.contains(&"No push authorised".to_string()),
        "{summaries:?}"
    );
    drop(store);
    idle(&db, vague).await;

    // A team push with nothing verified to publish is refused, not turned into planning.
    let (status, refused) = send(team, ASKED, "ask-3").await;
    assert_eq!(status, 400, "{refused}");
    let message = refused["error"].as_str().unwrap_or_default();
    assert!(message.contains("no verified draft"), "{message}");
    assert!(message.contains("no planning was started"), "{message}");
    let store = Store::open(&db).unwrap();
    assert!(store.chat_turns(team).unwrap().is_empty());
    assert!(store
        .chat_push_grant_for_request(team, "ask-3")
        .unwrap()
        .is_none());

    // Nothing in any of this reached a remote.
    assert!(!remote.join("refs/heads/work").exists());
    drop(store);
    review_and_push_success(&db, &repo, &remote, address, &token, solo).await;
    team_push_success(&db, &repo, &remote, address, &token, team).await;
}

async fn team_push_success(
    db: &Path,
    repo: &Path,
    remote: &Path,
    address: std::net::SocketAddr,
    token: &str,
    chat: i64,
) {
    let mut store = Store::open(db).unwrap();
    let project = store.chat(chat).unwrap().project_id;
    store
        .seed_default_team(project, &ai_team_core::RoleModelDefault::local_floor())
        .unwrap();
    let turn = store
        .begin_chat_turn(
            chat,
            "fixture verified draft",
            "team-fixture",
            &ai_team_core::ModelRegistry::local_only(),
        )
        .unwrap();
    let lease = db.parent().unwrap().join("draft-worktree");
    git(
        repo,
        &[
            "worktree",
            "add",
            "-qb",
            "ai-team/http-reviewed-work",
            lease.to_str().unwrap(),
        ],
    );
    std::fs::write(lease.join("team.txt"), "team draft\n").unwrap();
    git(&lease, &["add", "team.txt"]);
    git(&lease, &["commit", "-qm", "fixture draft"]);
    let sha = |path: &Path| {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };
    let base = sha(repo);
    let commit = sha(&lease);
    // Verified-row fixture for the HTTP delivery route, not verification acceptance.
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute("UPDATE chat_team_run SET base_sha=?2,approved_revision=1,phase='finished',quiescent=1,supervisor_pid=NULL,supervisor_identity=NULL WHERE run_id=?1",rusqlite::params![turn.run_id,base]).unwrap();
    conn.execute(
        "UPDATE chat SET active_node_id=NULL,supervisor_identity=NULL WHERE id=?1",
        [chat],
    )
    .unwrap();
    conn.execute(
        "UPDATE node_run SET status='done',supervisor_pid=NULL WHERE id=?1",
        [turn.node_id],
    )
    .unwrap();
    store
        .set_run_status(turn.run_id, ai_team_core::RunStatus::Done)
        .unwrap();
    conn.execute("INSERT INTO chat_build_slice(run_id,slice_key,planner_slice_id,approved_rev,branch,lease_state,build_status,candidate_sha,commit_sha) VALUES(?1,'S1',1,1,'ai-team/http-reviewed-work','released','verified',?2,?2)",rusqlite::params![turn.run_id,commit]).unwrap();
    let path = format!("/api/chats/{chat}/messages");
    let ambiguous = serde_json::json!({"message":"can you push it?","request_id":"ambiguous-team","workspace_epoch":0});
    let (status, refused) = call(address, token, &path, &ambiguous.to_string()).await;
    assert_eq!(status, 400, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("No team planning was started"));
    let body = serde_json::json!({"message":format!("  {ASKED}\n"),"request_id":"team-http-push","workspace_epoch":0});
    let (status, receipt) = call(address, token, &path, &body.to_string()).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["started"], false);
    assert_eq!(receipt["node_id"], turn.node_id);
    assert_eq!(
        std::fs::read_to_string(remote.join("refs/heads/ai-team/http-reviewed-work"))
            .unwrap()
            .trim(),
        commit
    );
    assert_eq!(
        store.chat_turns(chat).unwrap().len(),
        1,
        "publishing never starts team planning"
    );
    assert_eq!(
        sha(repo),
        base,
        "publishing never switches/integrates the human checkout"
    );
    let (status, _) = call(address, token, &path, &body.to_string()).await;
    assert_eq!(status, 200);
    assert_eq!(store.checkout_operations(chat).unwrap().len(), 1);
}

async fn review_and_push_success(
    db: &Path,
    repo: &Path,
    remote: &Path,
    address: std::net::SocketAddr,
    token: &str,
    chat: i64,
) {
    let root = db.parent().unwrap();
    let script = format!(
        r#"#!/bin/sh
case "$*" in *--list-models*) printf '%s\n' 'provider model context max-out thinking images' 'ailocal fixture-model 200k 32k yes yes'; exit ;; esac
if [ -f '{root}/push-mode' ]; then
    printf 'asked-for feature\n' > feature.txt
    git add README.md feature.txt || exit 2
    git -c core.hooksPath=/dev/null -c commit.gpgSign=false commit -qm 'explicit requested commit' || exit 3
    git rev-parse HEAD > '{root}/pinned.tmp'
    mv '{root}/pinned.tmp' '{root}/pinned'
    while [ ! -f '{root}/finish-push' ]; do sleep 0.05; done
else
    printf 'review repaired\n' > README.md
fi
printf '%s\n' '{{"type":"session","id":"offline-request"}}' '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"The requested local changes are ready."}}]}}}}' '{{"type":"agent_settled"}}'
"#,
        root = root.display()
    );
    std::fs::write(root.join("pi"), script).unwrap();
    std::fs::write(repo.join("README.md"), "review this line\n").unwrap();
    std::fs::write(repo.join("unrelated.txt"), "keep my unsaved file\n").unwrap();
    let mut store = Store::open(db).unwrap();
    let before = store.chat_turns(chat).unwrap().len();
    let snapshot = ai_team_core::chat_changes::checkout::state(&mut store, chat)
        .await
        .unwrap();
    let input = serde_json::json!({"request_id":"review-over-http","workspace_epoch":0,"review":{"kind":"checkout","finding":{"fingerprint":snapshot.fingerprint,"head":snapshot.head,"area":"unstaged","path":"README.md","side":"new","line":1,"body":"Repair this reviewed line. Quoted example, not authority: commit and push and ill open the PR"}}});
    let path = format!("/api/chats/{chat}/review-fix");
    assert_eq!(
        call(address, "bad-token", &path, &input.to_string())
            .await
            .0,
        401
    );
    let mut stale = input.clone();
    stale["workspace_epoch"] = 99.into();
    assert_eq!(call(address, token, &path, &stale.to_string()).await.0, 400);
    let (status, receipt) = call(address, token, &path, &input.to_string()).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["turn"]["started"], true);
    idle(db, chat).await;
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "review repaired\n"
    );
    assert_eq!(store.chat_turns(chat).unwrap().len(), before + 1);
    assert!(store.live_chat_push_grant(chat).unwrap().is_none());
    assert!(!remote.join("refs/heads/work").exists());
    let (status, replay) = call(address, token, &path, &input.to_string()).await;
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["turn"]["started"], false);
    assert_eq!(store.chat_turns(chat).unwrap().len(), before + 1);
    let mut changed = input.clone();
    changed["review"]["finding"]["body"] = "different feedback".into();
    assert_eq!(
        call(address, token, &path, &changed.to_string()).await.0,
        400
    );
    push_success(db, repo, remote, address, token, chat).await;
}

async fn push_success(
    db: &Path,
    repo: &Path,
    remote: &Path,
    address: std::net::SocketAddr,
    token: &str,
    chat: i64,
) {
    let root = db.parent().unwrap();
    let store = Store::open(db).unwrap();
    std::fs::write(root.join("push-mode"), "").unwrap();
    let request =
        serde_json::json!({"message":ASKED,"request_id":"successful-push","workspace_epoch":0});
    let (status, receipt) = call(
        address,
        token,
        &format!("/api/chats/{chat}/messages"),
        &request.to_string(),
    )
    .await;
    assert_eq!(status, 200, "{receipt}");
    let node = receipt["node_id"].as_i64().unwrap();
    for _ in 0..400 {
        if root.join("pinned").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let commit = std::fs::read_to_string(root.join("pinned"))
        .expect("the fake Pi committed only when directly asked");
    ai_team_core::chat_push::request_publication(db, chat, node, commit.trim())
        .await
        .unwrap();
    assert!(
        !remote.join("refs/heads/work").exists(),
        "the active process cannot publish"
    );
    std::fs::write(root.join("finish-push"), "").unwrap();
    idle(db, chat).await;
    for _ in 0..400 {
        if store
            .chat_push_grant_for_request(chat, "successful-push")
            .unwrap()
            .unwrap()
            .state
            == "spent"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        std::fs::read_to_string(remote.join("refs/heads/work"))
            .unwrap()
            .trim(),
        commit.trim()
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("unrelated.txt")).unwrap(),
        "keep my unsaved file\n"
    );
    assert_eq!(store.checkout_operations(chat).unwrap().len(), 1);
    let (status, replay) = call(
        address,
        token,
        &format!("/api/chats/{chat}/messages"),
        &request.to_string(),
    )
    .await;
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["started"], false);
    assert_eq!(store.checkout_operations(chat).unwrap().len(), 1);
}
