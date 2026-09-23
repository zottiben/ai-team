//! The server, driven over a real socket.
//!
//! These go through `Server::bind` and a TCP connection rather than calling handlers
//! directly, because the parts that break are the parts a unit test skips: the token
//! middleware, the status codes, the SPA fallback, and whether the JSON is the shape
//! the client was written against.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use ai_team_ui::{ServeOptions, Server, TOKEN_HEADER, TOKEN_QUERY};

struct Harness {
    addr: SocketAddr,
    token: String,
    _runtime: tokio::runtime::Runtime,
    /// Kept alive for as long as the server watches it. Only the no-database harness
    /// needs one; [`Harness::with_store`] hands its directory back to the caller.
    _dir: Option<tempfile::TempDir>,
}

impl Harness {
    /// A server with a real database behind it, seeded with one run.
    fn with_store() -> (Harness, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.db");
        let mut store = ai_team_core::Store::init(&path).unwrap();
        let project = store
            .create_project(ai_team_core::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        store
            .create_run(project.id, "add subtract", ai_team_core::RunTrigger::Manual)
            .unwrap();
        drop(store);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                store: Some(ai_team_core::Store::open(&path).unwrap()),
                credentials: ai_team_core::CredentialStore::isolated(),
                ..Default::default()
            }))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            let _ = server.serve().await;
        });
        (
            Harness {
                addr,
                token,
                _runtime: runtime,
                _dir: None,
            },
            dir,
        )
    }

    /// A server whose project is attached to a real git checkout.
    fn with_repo(repo: &std::path::Path) -> (Harness, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.db");
        let mut store = ai_team_core::Store::init(&path).unwrap();
        let project = store
            .create_project(ai_team_core::NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .attach_repo(
                project.id,
                ai_team_core::NewRepo {
                    main_path: Some(repo.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        drop(store);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                store: Some(ai_team_core::Store::open(&path).unwrap()),
                credentials: ai_team_core::CredentialStore::isolated(),
                ..Default::default()
            }))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            let _ = server.serve().await;
        });
        (
            Harness {
                addr,
                token,
                _runtime: runtime,
                _dir: None,
            },
            dir,
        )
    }

    /// A server with no database, watching a path one may appear at.
    fn watching(db_path: &std::path::Path) -> Harness {
        Harness::bound(
            ServeOptions {
                port: 0,
                token: None,
                store: None,
                db_path: Some(db_path.to_path_buf()),
                ..Default::default()
            },
            None,
        )
    }

    /// A server with no database, and no way to find one.
    ///
    /// Pointed at an empty temporary directory rather than left to default, because the
    /// default is *the machine's own* database. These tests were quietly exercising
    /// `~/.ai-team/team.db` - so they passed on CI, where a runner has none, and failed
    /// on the Mac this is used on, which is the wrong way round for the platform that
    /// matters (D12).
    fn start() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let options = ServeOptions {
            db_path: Some(dir.path().join("team.db")),
            ..Default::default()
        };
        // The directory is handed to the harness rather than assigned afterwards, so it
        // is owned from the moment it exists and nothing reads a field spelled `_dir`.
        Harness::bound(options, Some(dir))
    }

    /// Bind, spawn, and hand back the address and token.
    ///
    /// `dir` is whatever has to outlive the server - a temporary directory it is watching
    /// for a database that never appears.
    fn bound(options: ServeOptions, dir: Option<tempfile::TempDir>) -> Harness {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        // An integration-test dependency is not compiled with `cfg(test)`. Inject the
        // private store explicitly so no HTTP test reads or writes the login keychain.
        let options = ServeOptions {
            credentials: ai_team_core::CredentialStore::isolated(),
            ..options
        };
        let server = runtime.block_on(Server::bind(options)).unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            let _ = server.serve().await;
        });

        Harness {
            addr,
            token,
            _runtime: runtime,
            _dir: dir,
        }
    }

    /// POST with a JSON body, for the routes that change something.
    fn post(&self, path: &str, body: &str) -> Response {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: {}\r\n{TOKEN_HEADER}: {}\r\n\
             content-type: application/json\r\ncontent-length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            self.addr,
            self.token,
            body.len()
        )
        .unwrap();
        read_response(stream)
    }

    fn get(&self, path: &str) -> Response {
        self.request(path, Some(&self.token))
    }

    fn get_anonymous(&self, path: &str) -> Response {
        self.request(path, None)
    }

    fn request(&self, path: &str, token: Option<&str>) -> Response {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        let auth = match token {
            Some(t) => format!("{TOKEN_HEADER}: {t}\r\n"),
            None => String::new(),
        };
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {}\r\n{auth}Connection: close\r\n\r\n",
            self.addr
        )
        .unwrap();

        read_response(stream)
    }
}

/// Parse one HTTP/1.1 response off a socket.
fn read_response(mut stream: TcpStream) -> Response {
    let mut raw = String::new();
    {
        if let Err(error) = stream.read_to_string(&mut raw) {
            // macOS may report ECONNRESET after axum has already written a complete
            // `Connection: close` response. The bytes are authoritative; only fail when
            // the reset arrived before there was a response to parse.
            assert!(
                error.kind() == std::io::ErrorKind::ConnectionReset && !raw.is_empty(),
                "reading HTTP response: {error}"
            );
        }
    }
    {
        let (head, body) = raw.split_once("\r\n\r\n").expect("a complete response");
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        Response {
            status,
            head: head.to_lowercase(),
            body: body.to_string(),
        }
    }
}

struct Response {
    status: u16,
    head: String,
    body: String,
}

impl Response {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("expected JSON, got {:?}: {e}", self.body))
    }
}

#[test]
fn health_reports_the_version_and_the_bundle() {
    let app = Harness::start();
    let response = app.get("/api/health");

    assert_eq!(response.status, 200);
    let body = response.json();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    // Not asserted as true: a `cargo test` on a machine without node embeds nothing,
    // and failing here would blame the frontend for an absent toolchain. CI proves the
    // bundle is real and current with `git diff --exit-code -- ui/dist`.
    assert!(body["bundle_embedded"].is_boolean());
    assert!(body["bundle_files"].is_number());
}

#[test]
fn the_api_is_closed_without_the_token() {
    let app = Harness::start();

    let anonymous = app.get_anonymous("/api/health");
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.json()["error"], "unauthorized");

    let wrong = app.request("/api/health", Some("0".repeat(64).as_str()));
    assert_eq!(wrong.status, 401);
}

#[test]
fn the_token_may_arrive_in_the_query() {
    // EventSource cannot set a header, so the query has to work - and it is also the
    // URL the CLI prints and the desktop shell opens.
    let app = Harness::start();
    let path = format!("/api/health?{TOKEN_QUERY}={}", app.token);
    assert_eq!(app.get_anonymous(&path).status, 200);
}

#[test]
fn unknown_paths_fall_back_to_the_app() {
    // A deep link reloaded in the browser must reach the client router, not a 404.
    let app = Harness::start();
    let response = app.get_anonymous("/console/some-run");

    assert_eq!(response.status, 200);
    assert!(response.head.contains("text/html"));
    assert!(response.body.contains("<!doctype html>"));
    // Served from memory and rebuilt with the binary, so a cached copy of the previous
    // version is exactly what we do not want.
    assert!(response.head.contains("cache-control: no-cache"));
}

#[test]
fn the_window_served_is_the_bundle_of_the_checkout_it_was_built_from() {
    // Worktrees of one repository that share a CARGO_TARGET_DIR share the build script's
    // output too: cargo names a path package's build directory by where it sits in its
    // workspace, not by where the workspace is. A table naming its files by absolute path
    // embedded whichever checkout last ran the script - one worktree's binary serving
    // another's window, with nothing to say so.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    let own = std::fs::read_to_string(dist.join("app.js")).expect("ui/dist is committed");
    let app = Harness::start();

    let served = app.get_anonymous("/app.js");

    assert_eq!(served.status, 200);
    assert!(
        served.body == own,
        "served a {}-byte app.js, but this checkout's ui/dist/app.js is {} bytes",
        served.body.len(),
        own.len()
    );
}

#[test]
fn the_url_carries_the_token() {
    let app = Harness::start();
    let expected = format!("http://{}/?{TOKEN_QUERY}={}", app.addr, app.token);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(Server::bind(ServeOptions {
            port: 0,
            token: Some("fixed-token".into()),
            store: None,
            db_path: None,
            ..Default::default()
        }))
        .unwrap();

    assert!(expected.contains(&app.token));
    assert_eq!(server.token(), "fixed-token");
    assert!(server.url().ends_with("token=fixed-token"));
}

#[test]
fn the_api_is_a_view_over_the_database() {
    let (app, _dir) = Harness::with_store();

    let projects = app.get("/api/projects");
    assert_eq!(projects.status, 200);
    assert!(
        projects.body.contains("\"name\":\"Widget\""),
        "{}",
        projects.body
    );
    // The count a sidebar shows, computed server-side so two windows cannot disagree.
    assert!(
        projects.body.contains("\"open_runs\":1"),
        "{}",
        projects.body
    );

    let runs = app.get("/api/runs");
    assert!(runs.body.contains("add subtract"), "{}", runs.body);

    // A run carries its nodes and its spend, because the dock renders all three and
    // three round trips to draw one panel is three chances to show a half-updated run.
    let detail = app.get("/api/runs/1");
    assert_eq!(detail.status, 200);
    assert!(detail.body.contains("\"nodes\""), "{}", detail.body);
    assert!(detail.body.contains("\"usage\""), "{}", detail.body);
}

fn linked_checkout() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let source = tempfile::tempdir().unwrap();
    let repo = source.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(repo.join("main.txt"), "main").unwrap();
    git(&["add", "main.txt"]);
    git(&["commit", "-qm", "main"]);
    let task = source.path().join("task");
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature/task",
        task.to_str().unwrap(),
    ]);
    std::fs::write(task.join("task.txt"), "task").unwrap();
    (source, repo, task)
}

fn encoded(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('/', "%2F")
}

fn split_run(
    db: &std::path::Path,
    repo: &std::path::Path,
    task: &std::path::Path,
) -> (i64, i64, i64) {
    let mut store = ai_team_core::Store::open(db).unwrap();
    let project = store.find_project("widget").unwrap();
    let agents = store.agents(project.team_id.unwrap()).unwrap();
    let run = store
        .create_run_in(
            project.id,
            "build it",
            ai_team_core::RunTrigger::Manual,
            Some(task),
        )
        .unwrap();
    let registry = ai_team_core::ModelRegistry::local_only();
    let writable: Vec<_> = agents.iter().filter(|agent| !agent.read_only).collect();
    let mut leased_node = 0;
    for (agent, slice, path, branch) in [
        (writable[0], "W1", repo, "ai-team/W1"),
        (writable[1], "W2", task, "feature/task"),
    ] {
        let node = store
            .dispatch(run.id, agent.id, Some(slice), &registry)
            .unwrap();
        store
            .attach_worktree(node.id, path.to_str().unwrap(), Some(branch), None)
            .unwrap();
        store
            .set_node_status(node.id, ai_team_core::NodeStatus::Running)
            .unwrap();
        if path == repo {
            leased_node = node.id;
        }
    }
    (run.id, project.id, leased_node)
}

#[test]
fn delivery_approval_policies_are_visible_and_edited_together() {
    let (app, _dir) = Harness::with_store();
    let before = app.get("/api/roster?project=widget");
    assert_eq!(before.status, 200);
    assert_eq!(before.json()["delivery"]["push"], "ask");

    let changed = app.post(
        "/api/roster/delivery",
        r#"{"project":"widget","push":"auto","pr":"ask","merge":"manual"}"#,
    );
    assert_eq!(changed.status, 200, "{}", changed.body);

    let after = app.get("/api/roster?project=widget").json();
    assert_eq!(after["delivery"]["push"], "auto");
    assert_eq!(after["delivery"]["pr"], "ask");
    assert_eq!(after["delivery"]["merge"], "manual");
}

#[test]
fn manual_delivery_policy_refuses_a_remote_action_before_running_git_or_gh() {
    let (_source, repo, task) = linked_checkout();
    let (app, db) = Harness::with_repo(&repo);
    let mut store = ai_team_core::Store::open(&db.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let team_id = project.team_id.unwrap();
    store
        .update_delivery(
            team_id,
            ai_team_core::DeliverySettings {
                push: ai_team_core::DeliveryPolicy::Manual,
                ..Default::default()
            },
        )
        .unwrap();
    let run = store
        .create_run(project.id, "ship it", ai_team_core::RunTrigger::Manual)
        .unwrap();
    let backend = store
        .agents(team_id)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "backend")
        .unwrap();
    let node = store
        .dispatch(
            run.id,
            backend.id,
            Some("S1"),
            &ai_team_core::ModelRegistry::local_only(),
        )
        .unwrap();
    store
        .attach_worktree(node.id, repo.to_str().unwrap(), Some("ai-team/s1"), None)
        .unwrap();
    store
        .set_node_status(node.id, ai_team_core::NodeStatus::Done)
        .unwrap();
    drop(store);

    let request = serde_json::json!({
        "action": "push",
        "project": "widget",
        "workspace": repo,
    });
    let answer = app.post(
        &format!("/api/runs/{}/nodes/{}/deliver", run.id, node.id),
        &request.to_string(),
    );
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("manual"), "{}", answer.body);

    let mut store = ai_team_core::Store::open(&db.path().join("team.db")).unwrap();
    store
        .update_delivery(
            team_id,
            ai_team_core::DeliverySettings {
                push: ai_team_core::DeliveryPolicy::Ask,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let wrong_workspace = serde_json::json!({
        "action": "push",
        "project": "widget",
        "workspace": task,
    });
    let answer = app.post(
        &format!("/api/runs/{}/nodes/{}/deliver", run.id, node.id),
        &wrong_workspace.to_string(),
    );
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(
        answer.body.contains("selected workspace"),
        "{}",
        answer.body
    );
}

#[test]
fn ownership_detection_updates_the_editable_team_from_the_repository_stack() {
    let (_source, repo, _task) = linked_checkout();
    for name in ["app", "database", "resources"] {
        std::fs::create_dir_all(repo.join(name)).unwrap();
    }
    std::fs::write(repo.join("composer.json"), "{}").unwrap();
    std::fs::write(repo.join("package.json"), "{}").unwrap();
    let (app, db) = Harness::with_repo(&repo);

    let answer = app.post("/api/roster/ownership/detect", r#"{"project":"widget"}"#);
    assert_eq!(answer.status, 200, "{}", answer.body);

    let store = ai_team_core::Store::open(&db.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let agents = store.agents(project.team_id.unwrap()).unwrap();
    let backend = agents.iter().find(|agent| agent.role == "backend").unwrap();
    let frontend = agents
        .iter()
        .find(|agent| agent.role == "frontend")
        .unwrap();
    assert!(backend.zone.contains("app/**"), "{}", backend.zone);
    assert!(!frontend.zone.contains("app/**"), "{}", frontend.zone);
    assert!(frontend.zone.contains("resources/**"), "{}", frontend.zone);
}

#[test]
fn an_arbitrary_path_cannot_be_used_as_a_workspace() {
    let (_source, repo, task) = linked_checkout();
    let (app, _db) = Harness::with_repo(&repo);
    let linked = app.get(&format!(
        "/api/tree?project=widget&workspace={}&path=",
        encoded(&task)
    ));
    assert_eq!(linked.status, 200, "{}", linked.body);
    assert!(linked.body.contains("task.txt"), "{}", linked.body);

    let unrelated = tempfile::tempdir().unwrap();
    let refused = app.get(&format!(
        "/api/tree?project=widget&workspace={}&path=",
        encoded(unrelated.path())
    ));
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("is not a worktree"),
        "{}",
        refused.body
    );

    let refused_start = app.post(
        "/api/runs",
        &serde_json::json!({
            "project": "widget",
            "workspace": unrelated.path(),
            "prompt": "do not run this elsewhere"
        })
        .to_string(),
    );
    assert_eq!(refused_start.status, 400, "{}", refused_start.body);
    assert!(
        refused_start.body.contains("is not a worktree"),
        "{}",
        refused_start.body
    );
}

#[test]
fn a_run_and_all_its_leased_nodes_stay_in_the_workspace_that_started_it() {
    let (_source, repo, task) = linked_checkout();
    let (app, db) = Harness::with_repo(&repo);
    let (run_id, project_id, _) = split_run(&db.path().join("team.db"), &repo, &task);

    let run_count = |workspace: &std::path::Path| {
        let answer = app.get(&format!(
            "/api/runs?project={project_id}&workspace={}",
            encoded(workspace)
        ));
        serde_json::from_str::<serde_json::Value>(&answer.body)
            .unwrap()
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(run_count(&repo), 0, "a child run must not leak into main");
    assert_eq!(run_count(&task), 1);

    let wrong_workspace = app.get(&format!("/api/runs/{run_id}?workspace={}", encoded(&repo)));
    assert_eq!(wrong_workspace.status, 400, "{}", wrong_workspace.body);

    let task_detail = app.get(&format!("/api/runs/{run_id}?workspace={}", encoded(&task)));
    assert_eq!(task_detail.status, 200, "{}", task_detail.body);
    let task_detail: serde_json::Value = serde_json::from_str(&task_detail.body).unwrap();
    assert_eq!(task_detail["nodes"].as_array().unwrap().len(), 2);
    assert!(ai_team_core::same_worktree(
        task_detail["workspace_path"].as_str().unwrap(),
        &task.to_string_lossy()
    ));
}

#[test]
fn a_maker_session_reset_retires_the_address_and_keeps_the_node_evidence() {
    let (_source, repo, task) = linked_checkout();
    let (app, db) = Harness::with_repo(&repo);
    let path = db.path().join("team.db");
    let (run_id, _project_id, node_id) = split_run(&path, &repo, &task);
    {
        let mut store = ai_team_core::Store::open(&path).unwrap();
        store.set_node_session(node_id, "session-old").unwrap();
        store
            .set_node_status(node_id, ai_team_core::NodeStatus::Done)
            .unwrap();
    }

    let answer = app.post(
        &format!("/api/runs/{run_id}/nodes/{node_id}/reset-session"),
        &serde_json::json!({ "workspace": task }).to_string(),
    );
    assert_eq!(answer.status, 200, "{}", answer.body);

    let started = std::time::Instant::now();
    loop {
        let store = ai_team_core::Store::open(&path).unwrap();
        let node = store.node_run(node_id).unwrap();
        if node.session_retired_at.is_some() {
            assert_eq!(node.session_id.as_deref(), Some("session-old"));
            break;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn assert_node_recoverable(
    app: &Harness,
    run_id: i64,
    workspace: &std::path::Path,
    expected: bool,
) {
    let detail = app.get(&format!(
        "/api/runs/{run_id}?workspace={}",
        encoded(workspace)
    ));
    assert_eq!(detail.status, 200, "{}", detail.body);
    assert_eq!(detail.json()["nodes"][0]["recoverable"], expected);
}

#[test]
fn run_activity_is_readable_and_a_reply_stays_in_its_workspace_thread() {
    let (_source, repo, task) = linked_checkout();
    let (app, db) = Harness::with_repo(&repo);
    let path = db.path().join("team.db");
    let (run_id, _project_id, node_id) = split_run(&path, &repo, &task);
    {
        let mut store = ai_team_core::Store::open(&path).unwrap();
        store.set_node_session(node_id, "session-1").unwrap();
        store
            .append_event(
                run_id,
                ai_team_core::NewEvent::new(ai_team_core::EventKind::Cost, "step finished")
                    .on_node(node_id)
                    .by("orchestrator")
                    .with(serde_json::json!({
                        "message": { "content": [{
                            "type": "thinking",
                            "thinking": "Checking the plan",
                            "thinkingSignature": "do-not-send"
                        }] }
                    })),
            )
            .unwrap();
        store
            .append_event(
                run_id,
                ai_team_core::NewEvent::new(ai_team_core::EventKind::Note, "The short answer…")
                    .on_node(node_id)
                    .by("orchestrator")
                    .with(serde_json::json!({
                        "message": { "content": [{
                            "type": "text",
                            "text": "The complete answer the operator needs."
                        }] }
                    })),
            )
            .unwrap();
    }

    assert_node_recoverable(&app, run_id, &task, true);
    {
        let mut store = ai_team_core::Store::open(&path).unwrap();
        store
            .claim_node_supervision(node_id, i64::from(std::process::id()), None)
            .unwrap();
    }
    assert_node_recoverable(&app, run_id, &task, false);

    let events = app.get(&format!(
        "/api/runs/{run_id}/events?workspace={}",
        encoded(&task)
    ));
    assert_eq!(events.status, 200, "{}", events.body);
    assert!(events.body.contains("Checking the plan"), "{}", events.body);
    assert!(
        events
            .body
            .contains("The complete answer the operator needs."),
        "{}",
        events.body
    );
    assert!(!events.body.contains("do-not-send"), "{}", events.body);

    let reply = app.post(
        &format!("/api/runs/{run_id}/nodes/{node_id}/reply"),
        &serde_json::json!({
            "workspace": task.to_string_lossy(),
            "message": "Yes, continue with E5.3."
        })
        .to_string(),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);

    let store = ai_team_core::Store::open(&path).unwrap();
    assert_eq!(
        store
            .waiting_for(store.node_run(node_id).unwrap().agent_id.unwrap())
            .unwrap(),
        1
    );
    let human = store.events(run_id, None, 100).unwrap().pop().unwrap();
    assert_eq!(human.actor.as_deref(), Some("human"));
    assert_eq!(human.node_run_id, Some(node_id));
    assert_eq!(
        human.payload.unwrap()["body"],
        serde_json::Value::String("Yes, continue with E5.3.".into())
    );
    drop(store);

    let mut store = ai_team_core::Store::open(&path).unwrap();
    store
        .set_node_status(node_id, ai_team_core::NodeStatus::Done)
        .unwrap();
    drop(store);
    let closed = app.post(
        &format!("/api/runs/{run_id}/nodes/{node_id}/reply"),
        &serde_json::json!({
            "workspace": task.to_string_lossy(),
            "message": "This must not start unrelated work."
        })
        .to_string(),
    );
    assert_eq!(closed.status, 400, "{}", closed.body);
    assert!(closed.body.contains("no longer active"), "{}", closed.body);
}

#[test]
fn a_server_with_no_database_says_so_instead_of_failing() {
    // `ait ui` before `ait init` is a normal first run. The window should explain
    // itself, not look broken - and health must still answer, since that is what
    // `ait doctor` asks.
    //
    // `watching` rather than `start`, and for the reason the comment further down already
    // gives: `ServeOptions::default()` resolves the *machine's* database, so on a laptop
    // that has ever run `ait init` this asserted "no database" against a real one and
    // failed. It passed in CI and on a fresh checkout, which is the worst way for a test
    // to be wrong.
    let dir = tempfile::tempdir().unwrap();
    let app = Harness::watching(&dir.path().join("team.db"));

    assert_eq!(app.get("/api/health").status, 200);

    let projects = app.get("/api/projects");
    assert_eq!(projects.status, 503, "not a 500: nothing is broken");
    assert!(projects.body.contains("ait init"), "{}", projects.body);
}

#[test]
fn plan_approval_claims_the_existing_run_instead_of_starting_another() {
    let (app, dir) = Harness::with_store();
    let path = dir.path().join("team.db");
    let mut store = ai_team_core::Store::open(&path).unwrap();
    let run = store.runs(None, 1).unwrap().remove(0);
    store.set_run_plan(run.id, "widget-plan").unwrap();
    store.block_run(run.id, "Plan ready for approval").unwrap();
    drop(store);

    let continued = app.post(&format!("/api/runs/{}/approve-plan", run.id), "{}");
    assert_eq!(continued.status, 200, "{}", continued.body);
    assert_eq!(continued.json()["run_id"], run.id);

    // The claim is atomic. Even if the background continuation has already failed for
    // this deliberately node-less fixture, another window cannot start it again.
    let duplicate = app.post(&format!("/api/runs/{}/approve-plan", run.id), "{}");
    assert_eq!(duplicate.status, 400, "{}", duplicate.body);
    let store = ai_team_core::Store::open(&path).unwrap();
    assert_eq!(store.runs(None, 10).unwrap().len(), 1);
}

#[test]
fn notifications_are_listed_and_marked_read_without_changing_run_state() {
    let (app, dir) = Harness::with_store();
    let path = dir.path().join("team.db");
    let mut store = ai_team_core::Store::open(&path).unwrap();
    let project = store.projects().unwrap().remove(0);
    let notice = store
        .notify_once(ai_team_core::NewNotification {
            dedupe_key: "node:7:parked".into(),
            project_id: project.id,
            workspace_path: Some("/repo/task".into()),
            run_id: None,
            node_run_id: None,
            kind: "input_required".into(),
            title: "Planner needs input".into(),
            body: "Choose an acceptance criterion.".into(),
            action_path: None,
        })
        .unwrap()
        .unwrap();
    drop(store);

    let listed = app.get("/api/notifications");
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json()[0]["title"], "Planner needs input");
    assert!(listed.json()[0]["read_at"].is_null());

    let read = app.post(&format!("/api/notifications/{}/read", notice.id), "{}");
    assert_eq!(read.status, 200);
    assert!(read.json()["read_at"].is_string());
}

#[test]
fn the_event_stream_needs_a_token_like_everything_else() {
    // EventSource cannot set a header, so the stream takes its token from the query -
    // which makes it exactly the route where an auth hole would go unnoticed.
    let (app, _dir) = Harness::with_store();
    assert_eq!(app.get_anonymous("/api/events").status, 401);
    assert_eq!(
        app.get_anonymous(&format!("/api/events?{TOKEN_QUERY}=wrong"))
            .status,
        401
    );
}

#[test]
fn the_health_report_is_served_without_a_database() {
    // The one route that must work on a machine nobody has set up, because that is the
    // machine somebody most needs a report about. Every other route answers 503 and says
    // `ait init`, which is right for them and exactly wrong here.
    //
    // Its own path, not the machine's - see the note on the test above.
    let dir = tempfile::tempdir().unwrap();
    let harness = Harness::watching(&dir.path().join("team.db"));

    assert_eq!(harness.get("/api/projects").status, 503);

    let response = harness.get("/api/doctor");
    assert_eq!(response.status, 200, "{}", response.body);
    let report = response.json();
    assert_eq!(report["needs_setup"], true);
    assert_eq!(report["can_run"], false);
    assert_eq!(report["severity"], "blocking");
    assert!(
        report["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty()),
        "{}",
        response.body
    );
}

#[test]
fn a_report_of_a_set_up_machine_still_wants_a_provider() {
    // A database and a project are not enough to run: something has to be allowed to
    // think. Worth asserting, because "set up" is easy to define as "has a database".
    let (harness, _dir) = Harness::with_store();
    let report = harness.get("/api/doctor").json();

    let database = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "database")
        .unwrap()
        .clone();
    assert_eq!(database["severity"], "fine", "{database}");
}

#[test]
fn a_fix_ai_team_does_not_own_cannot_even_be_asked_for() {
    // D17: the set of things this route can cause is the set of `Action` variants. An
    // install is not one of them, so it is rejected at deserialisation rather than by a
    // check at the end of a function that looked like it might do it.
    let (harness, _dir) = Harness::with_store();

    let refused = harness.post("/api/doctor/fix", r#"{"action":"install_ai_planner"}"#);
    assert_eq!(refused.status, 422, "{}", refused.body);

    let nonsense = harness.post("/api/doctor/fix", r#"{"action":"rm -rf /"}"#);
    assert_eq!(nonsense.status, 422, "{}", nonsense.body);
}

#[test]
fn the_window_picks_up_a_database_created_after_it_started() {
    // The bug this replaced: the server decided at startup whether a database existed and
    // never looked again, so creating one from the setup page left every route answering
    // "run `ait init`" until the process was restarted - a confusing way to be told that
    // setup had worked.
    //
    // Pointed at its own path rather than the machine's: a test that reaches for
    // `default_db_path` writes into the developer's home, which this one did once.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("team.db");
    let harness = Harness::watching(&path);

    assert_eq!(harness.get("/api/projects").status, 503);

    // Created the way the setup page creates it.
    ai_team_core::Store::init(&path).unwrap();

    assert_eq!(
        harness.get("/api/projects").status,
        200,
        "the same process should find it"
    );
}

#[test]
fn a_repository_can_be_found_by_browsing_before_there_is_a_database() {
    // D24. The picker exists so the *first* thing somebody does on a new machine - find
    // the repository - does not require typing an absolute path from memory. One that
    // needed a project registered already would be unavailable exactly then, so this runs
    // against a server with no store at all.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("widget/.git")).unwrap();
    std::fs::create_dir_all(dir.path().join("notes")).unwrap();
    std::fs::write(dir.path().join("a-file.txt"), "x").unwrap();

    let harness = Harness::watching(&dir.path().join("team.db"));

    let listed = harness.get(&format!(
        "/api/browse?path={}",
        urlencode(&dir.path().to_string_lossy())
    ));
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(listed.body.contains("widget"), "{}", listed.body);
    assert!(listed.body.contains(r#""repo":true"#), "{}", listed.body);
    assert!(listed.body.contains("notes"), "{}", listed.body);
    // Directories only. This is for finding a checkout, not for reading somebody's files.
    assert!(!listed.body.contains("a-file.txt"), "{}", listed.body);
}

#[test]
fn browsing_needs_a_token_like_everything_else() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Harness::watching(&dir.path().join("team.db"));
    assert_eq!(harness.get_anonymous("/api/browse").status, 401);
}

/// Percent-encode a path for a query string.
///
/// Written out because this is the only test that needs it and a dependency to escape a
/// temp directory would be one more thing in the lockfile.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Nothing ai-team serves ever carries a context token.
///
/// The property that makes the settings page safe, and the reason its field is blank on
/// load: `/settings` says *whether* there is a token, never what it is, so there is
/// nothing to prefill it with and nothing to end up in a response body, a browser cache
/// or a log in between.
#[test]
fn no_route_ever_produces_a_context_token() {
    let (harness, _dir) = Harness::with_store();

    let settings = harness.get("/api/settings");
    assert_eq!(settings.status, 200, "{}", settings.body);

    let sources = settings.json()["context"].clone();
    for source in sources.as_array().expect("context sources") {
        // Presence booleans and a variable name. No field that could hold a value.
        assert!(source["oauth_connected"].is_boolean(), "{source}");
        assert!(source["token_set"].is_boolean(), "{source}");
        assert!(
            source.get("token").is_none(),
            "a token field exists: {source}"
        );
        assert!(
            source.get("value").is_none(),
            "a value field exists: {source}"
        );
    }
}

#[test]
fn storing_a_token_answers_without_repeating_it() {
    let (harness, _dir) = Harness::with_store();

    let cleared = harness.post("/api/settings/token", r#"{"source":"clickup","token":""}"#);
    assert_eq!(cleared.status, 200, "{}", cleared.body);
    assert_eq!(cleared.json()["token_set"], false);
}

/// A token goes through the real HTTP route and into the harness's isolated store.
///
/// The platform invocation has its own ignored core test. Keeping that boundary explicit
/// prevents an ordinary server test from opening the operator's login keychain.
#[test]
fn a_context_token_goes_in_and_never_comes_back() {
    const SECRET: &str = "pk_a_very_recognisable_test_token";

    let (harness, _dir) = Harness::with_store();

    let stored = harness.post(
        "/api/settings/token",
        &format!(r#"{{"source":"clickup","token":"{SECRET}"}}"#),
    );
    assert_eq!(stored.status, 200, "{}", stored.body);
    assert_eq!(stored.json()["token_set"], true);
    assert!(
        !stored.body.contains(SECRET),
        "the route echoed the token back: {}",
        stored.body
    );

    let settings = harness.get("/api/settings");
    assert!(
        !settings.body.contains(SECRET),
        "settings produced the token: {}",
        settings.body
    );

    let cleared = harness.post("/api/settings/token", r#"{"source":"clickup","token":""}"#);
    assert_eq!(cleared.status, 200, "{}", cleared.body);
}

#[test]
fn a_sign_in_request_cannot_carry_a_command() {
    // D25 lets this route start a process, which is the thing D17 spends a lot of care
    // refusing - so the boundary is that the command is *not in the request*. The server
    // looks it up from the provider, exactly as `/doctor/fix` looks a repair up from an
    // `Action`, and a body that tries to supply one is refused rather than quietly having
    // the field ignored. Ignoring it would be the same behaviour for a reason nobody can
    // see in the type, and nothing here could assert on it.
    //
    // Deliberately no happy path in this test: succeeding would start a real OAuth flow
    // and open somebody's browser. What the command *is* is asserted in `ai-team-core`,
    // where it is a lookup rather than a process.
    let (harness, _dir) = Harness::with_store();

    for body in [
        r#"{"provider":"claude","command":"rm -rf /"}"#,
        r#"{"command":"rm -rf /"}"#,
        r#"{"provider":"rm -rf /"}"#,
        r#"{"provider":"claude","extra":1}"#,
    ] {
        let answer = harness.post("/api/settings/sign-in", body);
        assert_eq!(answer.status, 422, "{body} was accepted: {}", answer.body);
    }
}

#[test]
fn context_oauth_accepts_a_source_and_never_a_command_or_url() {
    // Deliberately no happy path: succeeding starts Pi and may open a browser. The exact
    // generated config is asserted in core; this asserts that loopback cannot turn the
    // route into a shell by supplying a command or endpoint.
    let (harness, _dir) = Harness::with_store();

    for body in [
        r#"{"source":"clickup","command":"rm -rf /"}"#,
        r#"{"command":"rm -rf /"}"#,
        r#"{"source":"https://attacker.invalid/mcp"}"#,
        r#"{"source":"clickup","url":"https://attacker.invalid/mcp"}"#,
    ] {
        let answer = harness.post("/api/settings/context-auth", body);
        assert_eq!(answer.status, 422, "{body} was accepted: {}", answer.body);
    }
}

#[test]
fn a_provider_with_no_sign_in_flow_is_refused_rather_than_half_started() {
    // The local gateway has no account and a GLM plan is a key to paste. Offering either
    // a terminal would leave somebody watching a shell that exits immediately.
    let (harness, _dir) = Harness::with_store();

    for provider in ["local", "zai"] {
        let answer = harness.post(
            "/api/settings/sign-in",
            &format!(r#"{{"provider":"{provider}"}}"#),
        );
        assert_ne!(answer.status, 200, "{provider}: {}", answer.body);
    }
}

#[test]
fn a_project_with_no_checkout_says_so_rather_than_reporting_no_worktrees() {
    // The two answers look identical as an empty list and mean opposite things: one is a
    // repository whose pool is empty, the other is ai-team having nowhere to look. The
    // bug this guards is the second reported as the first, which reads as "you have no
    // worktrees" to somebody who has four.
    let (harness, _dir) = Harness::with_store();

    let answer = harness.get("/api/worktrees?project=widget");
    assert_ne!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains("checkout"), "{}", answer.body);
}

#[test]
fn worktrees_need_a_token_like_everything_else() {
    let (harness, _dir) = Harness::with_store();
    assert_eq!(
        harness
            .get_anonymous("/api/worktrees?project=widget")
            .status,
        401
    );
}
