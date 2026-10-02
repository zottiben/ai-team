//! Bounded HTTP -> controller -> offline Pi/Git lifecycle. This binary owns its env.
#![cfg(unix)]
use ai_team_core::{CredentialStore, NewProject, NewRepo, RoleModelDefault, Store};
use ai_team_ui::{ServeOptions, Server};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

struct Fixture {
    root: tempfile::TempDir,
    db: PathBuf,
    repo: PathBuf,
    addr: std::net::SocketAddr,
    token: String,
    runtime: tokio::runtime::Runtime,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let bin = path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
        std::env::set_var("HOME", &path);
        std::env::set_var("XDG_CONFIG_HOME", path.join("config"));
        std::env::set_var("AI_TEAM_HOME", path.join("state"));
        std::env::set_var("BUILD_TEST_ROOT", &path);
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("npm_config_update_notifier", "false");
        std::env::set_var("ANTHROPIC_API_KEY", "decoy-never-send");
        std::env::set_var("OPENAI_API_KEY", "decoy-never-send");
        std::fs::create_dir_all(path.join("config/ai-team")).unwrap();
        std::fs::write(
            path.join("config/ai-team/machine.toml"),
            ai_team_core::DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
        )
        .unwrap();
        for (name, script) in [
            ("pi", include_str!("fixtures/chat-lifecycle-pi.mjs")),
            (
                "awt",
                include_str!("../../ai-team-core/tests/fixtures/chat-worker-awt.mjs"),
            ),
        ] {
            std::fs::write(bin.join(format!("{name}.mjs")), script).unwrap();
            std::fs::write(
                bin.join(name),
                format!(
                    "#!/bin/sh\nexec node '{}' \"$@\"\n",
                    bin.join(format!("{name}.mjs")).display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        for name in ["aip", "claude", "codex", "security"] {
            std::fs::write(
                bin.join(name),
                "#!/bin/sh\ntouch \"$BUILD_TEST_ROOT/unexpected-neighbour\"\nexit 99\n",
            )
            .unwrap();
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(path.join("worker-mode"), "normal").unwrap();
        Self::with_runtime(root, path)
    }

    fn with_runtime(root: tempfile::TempDir, path: PathBuf) -> Self {
        let repo = path.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        std::fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"test":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        let db = path.join("team.db");
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "HTTP lifecycle".into(),
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
        let team = store
            .seed_default_team(project.id, &RoleModelDefault::local_floor())
            .unwrap();
        let mut rails = store.team(team.id).unwrap().guardrails;
        rails.max_repairs = 1;
        store
            .update_team(team.id, &team.name, &team.description, rails)
            .unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(Server::bind(ServeOptions {
                store: Some(store),
                credentials: CredentialStore::isolated(),
                ..Default::default()
            }))
            .unwrap();
        let addr = server.addr();
        let token = server.token().to_string();
        runtime.spawn(async move {
            server.serve().await.unwrap();
        });
        Self {
            root,
            db,
            repo,
            addr,
            token,
            runtime,
        }
    }
    fn request(&self, method: &str, route: &str, body: Value) -> (u16, Value) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let body = body.to_string();
        let request = format!("{method} /api{route} HTTP/1.1\r\nHost: {}\r\nx-ai-team-token: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nConnection: close\r\n\r\n{body}", self.addr, self.token, body.len());
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        if let Err(error) = stream.read_to_string(&mut raw) {
            assert!(
                error.kind() == std::io::ErrorKind::ConnectionReset && !raw.is_empty(),
                "{error}"
            );
        }
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    }
    fn post(&self, route: &str, body: Value) -> Value {
        let (status, response) = self.request("POST", route, body);
        assert_eq!(status, 200, "{route}: {response}");
        response
    }
    fn detail(&self, id: i64) -> Value {
        let (status, response) = self.request("GET", &format!("/chats/{id}"), Value::Null);
        assert_eq!(status, 200, "{response}");
        response
    }
    fn wait(&self, id: i64, predicate: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(45);
        loop {
            let detail = self.detail(id);
            if predicate(&detail) {
                return detail;
            }
            assert!(Instant::now() < until, "chat did not settle: {detail}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn mode(&self, value: &str) {
        std::fs::write(self.root.path().join("worker-mode"), value).unwrap();
    }
    fn send(&self, id: i64, request: &str) -> Value {
        self.post(
            &format!("/chats/{id}/messages"),
            json!({"message":request,"request_id":request}),
        )
    }
    fn switch(&self, id: i64, mode: &str) {
        self.post(
            &format!("/chats/{id}/mode"),
            json!({"mode":mode,"expect_revision":self.detail(id)["rev"]}),
        );
    }
    fn plan(&self, id: i64, mut action: Value) -> Value {
        let route = format!("/chats/{id}/plan");
        let (_, current) = self.request("GET", &route, Value::Null);
        action["expect_revision"] = current["revision"].clone();
        self.post(&route, action)
    }
    fn add_slice(&self, id: i64, key: &str) {
        self.plan(id, json!({"action":"add_slice","key":key,"title":"Bounded change","scope":"One fixture change","touches":["crates/**"],"demo":"fixture test"}));
        self.plan(
            id,
            json!({"action":"set_slice_status","key":key,"status":"ready"}),
        );
    }
    fn team(&self, id: i64, action: &str, body: Value) -> Value {
        self.post(&format!("/chats/{id}/team/{action}"), body)
    }
    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.path().join("pi-calls"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn target(detail: &Value) -> Value {
    detail["team_recovery"]["target"].clone()
}
fn approval(review: &Value) -> Value {
    let run = &review["execution"];
    json!({"target":{"chat_id":run["chat_id"],"run_id":run["run_id"],"node_id":run["control_node_id"],"expect_revision":run["rev"]},
        "approval":{"expect_control_revision":run["rev"],"expect_plan_revision":review["plan"]["revision"],"expect_roster_revision":review["roster_revision"],"expect_head":review["head"]}})
}

#[test]
fn authenticated_solo_team_stop_continue_close_and_concurrent_chats() {
    let f = Fixture::new();
    let linked = f.root.path().join("linked");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let create = |workspace: &Path| {
        f.post("/chats", json!({"project":"http-lifecycle","workspace":workspace,"provider":"local","model":"fixture","reasoning":"high"}))["id"].as_i64().unwrap()
    };
    let chat = create(&f.repo);
    let other = create(&linked);
    let collision = create(&f.repo);
    f.mode("hold-solo");
    let first = f.send(chat, "first solo");
    let second = f.send(other, "independent solo");
    f.wait(chat, |d| d["live_text"] == "Active assistant");
    f.wait(other, |d| d["live_text"] == "Active assistant");
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{collision}/messages"),
            json!({"message":"collision","request_id":"collision"})
        )
        .0,
        400
    );
    f.post(
        &format!("/chats/{chat}/stop"),
        json!({"node_id":first["node_id"]}),
    );
    f.wait(chat, |d| d["active_node_id"].is_null());
    assert_eq!(f.detail(other)["active_node_id"], second["node_id"]);
    std::fs::write(
        f.root.path().join(format!("release-{}", second["node_id"])),
        "go",
    )
    .unwrap();
    f.wait(other, |d| d["active_node_id"].is_null());
    f.mode("normal");
    f.send(chat, "solo followup");
    f.wait(chat, |d| d["active_node_id"].is_null());
    let solo_session = f.calls().last().unwrap()["session"].clone();
    build_and_return(&f, chat, other);
    assert_eq!(f.calls().last().unwrap()["session"], solo_session);
    failed_build_returns_to_actual_solo(&f, chat, &solo_session);
    assert_eq!(f.detail(other)["turns"].as_array().unwrap().len(), 1);
    assert!(!f.root.path().join("unexpected-neighbour").exists());
    assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    assert!(Store::open(&f.db)
        .unwrap()
        .chat_team_recovery_scan(None)
        .unwrap()
        .is_empty());
    drop(f.runtime);
}

fn build_and_return(f: &Fixture, chat: i64, other: i64) {
    let base = git(&f.repo, &["rev-parse", "HEAD"]);
    f.plan(chat, json!({"action":"create_plan","title":"HTTP plan"}));
    f.add_slice(chat, "S1");
    f.switch(chat, "team");
    f.send(chat, "plan the change");
    let paused = f.wait(chat, |d| d["state"] == "awaiting_approval");
    let review = f.team(chat, "review", target(&paused));
    let question = f.plan(chat, json!({"action":"open_question","body":"Proceed?"}));
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{chat}/team/approve"),
            approval(&review)
        )
        .0,
        400
    );
    let question_id = question["bundle"]["questions"][0]["id"].clone();
    f.plan(
        chat,
        json!({"action":"answer_question","question_id":question_id,"answer":"yes"}),
    );
    assert!(
        !f.root.path().join("pool.json").exists(),
        "answers must not acquire work"
    );
    let review = f.team(chat, "review", target(&f.detail(chat)));
    f.mode("slow-maker");
    f.team(chat, "approve", approval(&review));
    let building = f.wait(chat, |d| {
        d["turns"].as_array().unwrap().last().unwrap()["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["live_text"] == "Active backend")
    });
    f.team(chat, "stop", target(&building));
    let stopped = f.wait(chat, |d| {
        d["state"] == "team_blocked" && d["team_recovery"]["state"] == "quiescent"
    });
    let calls_before = f.calls().len();
    f.team(chat, "reconcile", target(&stopped));
    let stopped = f.detail(chat);
    assert_eq!(
        f.calls().len(),
        calls_before,
        "reconciliation must not start a model"
    );
    let slice = &stopped["team_builds"][0]["slices"][0];
    let lease = PathBuf::from(slice["worktree_path"].as_str().unwrap());
    assert!(lease.join("crates/S1.txt").exists());
    f.mode("normal");
    f.team(
        chat,
        "continue",
        json!({"target":target(&stopped),"slice_key":"S1","expect_slice_revision":slice["rev"]}),
    );
    let done = f.wait(chat, |d| d["active_node_id"].is_null());
    let commit = done["team_builds"][0]["slices"][0]["commit_sha"]
        .as_str()
        .unwrap();
    assert_eq!(
        git(&f.repo, &["show", &format!("{commit}:crates/S1.txt")]),
        "Built S1"
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), base);
    review_and_integrate(f, chat, other, commit);
    f.switch(chat, "single");
    f.send(chat, "solo after verified draft");
    f.wait(chat, |d| d["active_node_id"].is_null());
}

fn review_and_integrate(f: &Fixture, chat: i64, other: i64, commit: &str) {
    let calls = f.calls().len();
    let (status, changes) = f.request("GET", &format!("/chats/{chat}/changes"), Value::Null);
    assert_eq!(status, 200, "{changes}");
    let target = &changes["drafts"][0]["target"];
    let tree = f.post(&format!("/chats/{chat}/draft/tree"), target.clone());
    assert!(tree
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "crates/S1.txt"));
    let file = f.post(
        &format!("/chats/{chat}/draft/file"),
        json!({"target":target,"path":"crates/S1.txt"}),
    );
    assert_eq!(file["commit_sha"], commit);
    assert_eq!(file["text"].as_str().unwrap().trim(), "Built S1");
    let reviewed = f.post(&format!("/chats/{chat}/draft/review"), target.clone());
    assert_eq!(reviewed["draft"]["commit_sha"], commit);
    assert!(reviewed["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "crates/S1.txt"));
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{other}/draft/review"),
            target.clone()
        )
        .0,
        400
    );
    f.post(
        &format!("/chats/{chat}/draft/findings"),
        json!({"target":target,"body":"Reviewed the exact draft"}),
    );
    let preview = f.post(
        &format!("/chats/{chat}/delivery/preview"),
        json!({"target":target,"action":"integrate"}),
    );
    let approval = json!({"delivery_id":preview["id"],"expect_revision":preview["rev"]});
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{other}/delivery/approve"),
            approval.clone()
        )
        .0,
        400
    );
    std::fs::write(f.repo.join("solo-dirt.txt"), "preserve").unwrap();
    let refused = f.post(&format!("/chats/{chat}/delivery/approve"), approval);
    assert_eq!(refused["state"], "refused");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("solo-dirt.txt")).unwrap(),
        "preserve"
    );
    std::fs::remove_file(f.repo.join("solo-dirt.txt")).unwrap();
    let preview = f.post(
        &format!("/chats/{chat}/delivery/preview"),
        json!({"target":target,"action":"integrate"}),
    );
    let approval = json!({"delivery_id":preview["id"],"expect_revision":preview["rev"]});
    let done = f.post(&format!("/chats/{chat}/delivery/approve"), approval.clone());
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), commit);
    assert_eq!(
        f.post(&format!("/chats/{chat}/delivery/approve"), approval)["state"],
        "done"
    );
    assert_eq!(
        f.calls().len(),
        calls,
        "review and delivery must never run a model"
    );
}

fn refuses_candidate_restart(f: &Fixture, chat: i64, stopped: &Value) {
    let slice = &stopped["team_builds"].as_array().unwrap().last().unwrap()["slices"][0];
    let db = ai_team_core::Db::open(&f.db).unwrap();
    db.conn()
        .execute(
            "UPDATE chat_build_slice SET candidate_sha = 'recorded-candidate' WHERE run_id = ?1",
            [slice["run_id"].as_i64().unwrap()],
        )
        .unwrap();
    let request = json!({"target":target(stopped),"slice_key":slice["slice_key"],"expect_slice_revision":slice["rev"]});
    let (status, error) = f.request("POST", &format!("/chats/{chat}/team/continue"), request);
    assert_eq!(
        status, 400,
        "a refused continuation must not be acknowledged as accepted: {error}"
    );
    assert!(error["error"].as_str().unwrap().contains("reconciliation"));
    assert_eq!(target(&f.detail(chat)), target(stopped));
}

fn failed_build_returns_to_actual_solo(f: &Fixture, chat: i64, solo_session: &Value) {
    f.add_slice(chat, "S2");
    f.switch(chat, "team");
    f.send(chat, "plan another change");
    let paused = f.wait(chat, |d| d["state"] == "awaiting_approval");
    let review = f.team(chat, "review", target(&paused));
    f.mode("reject");
    f.team(chat, "approve", approval(&review));
    let failed = f.wait(chat, |d| {
        d["state"] == "team_blocked" && d["team_recovery"]["state"] == "quiescent"
    });
    refuses_candidate_restart(f, chat, &failed);
    let build = failed["team_builds"].as_array().unwrap().last().unwrap();
    let slice = &build["slices"][0];
    let kept = PathBuf::from(slice["worktree_path"].as_str().unwrap()).join("crates/S2.txt");
    let original = std::fs::read(&kept).unwrap();
    let inspect_target = json!({"run_id":slice["run_id"],"slice_key":"S2","revision":slice["rev"]});
    let inspected = f.post(
        &format!("/chats/{chat}/retained/inspect"),
        inspect_target.clone(),
    );
    assert_eq!(inspected["path"], slice["worktree_path"]);
    let file = f.post(
        &format!("/chats/{chat}/retained/file"),
        json!({"target":inspect_target,"path":"crates/S2.txt"}),
    );
    assert_eq!(file["text"].as_str().unwrap().trim(), "Built S2");
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{chat}/retained/file"),
            json!({"target":inspect_target,"path":"../../not-owned"})
        )
        .0,
        400
    );
    assert!(
        inspected["untracked"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "crates/S2.txt")
            || !inspected["staged"].as_array().unwrap().is_empty()
    );
    let outside = f.root.path().join("outside-retained.txt");
    std::fs::write(&outside, "must not be read").unwrap();
    let escape = kept.parent().unwrap().join("escape");
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    assert_eq!(
        f.request(
            "POST",
            &format!("/chats/{chat}/retained/file"),
            json!({"target":inspect_target,"path":"crates/escape"})
        )
        .0,
        400
    );
    std::fs::remove_file(escape).unwrap();
    let calls_before_keep = f.calls().len();
    f.post(
        &format!("/chats/{chat}/retained/keep"),
        json!({"target":inspect_target,"reason":"Keep inspected failure"}),
    );
    assert_eq!(f.calls().len(), calls_before_keep);
    assert_eq!(std::fs::read(&kept).unwrap(), original);
    let (_, plan) = f.request("GET", &format!("/chats/{chat}/plan"), Value::Null);
    f.team(chat, "close", json!({"target":target(&failed),"expect_plan_revision":plan["revision"],"expect_slices":{"S2":slice["rev"]},"reason":"Keep failed work for inspection"}));
    f.switch(chat, "single");
    f.mode("normal");
    f.send(chat, "continue solo after closed build");
    let done = f.wait(chat, |d| d["active_node_id"].is_null());
    assert_eq!(&f.calls().last().unwrap()["session"], solo_session);
    assert_eq!(std::fs::read(&kept).unwrap(), original);
    assert_eq!(
        done["team_builds"].as_array().unwrap().last().unwrap()["slices"][0]["lease_state"],
        "retained"
    );
}
