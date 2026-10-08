//! Real process/SQLite boundaries with fake Pi. No subscription, keychain or real home.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ai_team_core::{
    drive_chat, ChatSubmission, ModelRegistry, NewChat, NewProject, NodeStatus, Provider,
    Reasoning, Store,
};

fn git(repo: &Path, args: &[&str]) {
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
}

fn executable(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("the supervised turn did not reach the expected state");
}

struct Harness {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    db: PathBuf,
    store: Store,
    first: i64,
    other: i64,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let bin = root.join("bin");
        let repo = root.join("repo");
        let config = root.join("config");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(config.join("ai-team")).unwrap();
        std::fs::write(
            config.join("ai-team/machine.toml"),
            ai_team_core::DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
        )
        .unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("AI_TEAM_HOME", root.join("team-home"));
        std::env::set_var("CHAT_TEST_ROOT", &root);
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
        std::env::set_var("ANTHROPIC_API_KEY", "must-not-be-inherited");
        executable(
            &bin.join("aip"),
            "#!/bin/sh\nprintf called >> \"$CHAT_TEST_ROOT/planner-touched\"\nexit 91\n",
        );
        executable(
            &bin.join("awt"),
            "#!/bin/sh\nprintf called >> \"$CHAT_TEST_ROOT/lease-touched\"\nexit 92\n",
        );
        executable(&bin.join("pi"), include_str!("fixtures/chat-pi.sh"));
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
        std::fs::write(
            repo.join(".mcp.json"),
            serde_json::json!({"mcpServers": {
                "ai-team-planner": {"command":"shadow-planner"},
                "ai-planner": {"command":"aip"},
                "planner-alias": {"command":"/usr/local/bin/aip"},
                "file-sql": {"command":"file-sql"},
                "chrome-devtools": {"command":"fixture-browser"}
            }})
            .to_string(),
        )
        .unwrap();
        git(&repo, &["add", "README.md"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        let db = root.join("team.db");
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Chat fixture".into(),
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
        let new_chat = || NewChat {
            project_id: project.id,
            workspace: repo.clone(),
            provider: Provider::Local,
            model: "fixture-model".into(),
            reasoning: Reasoning::High,
        };
        let first = store.create_chat(new_chat()).unwrap().id;
        let other = store.create_chat(new_chat()).unwrap().id;
        Self {
            _dir: dir,
            root,
            repo,
            db,
            store,
            first,
            other,
        }
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.root.join("mode"), mode).unwrap();
    }

    fn submit(&mut self, chat: i64, message: &str) -> ChatSubmission {
        self.store
            .begin_chat_turn(chat, message, message, &ModelRegistry::load().unwrap())
            .unwrap()
    }

    async fn complete(&mut self, chat: i64, message: &str) -> i64 {
        let turn = self.submit(chat, message);
        drive_chat(&self.db, chat, turn.node_id, false)
            .await
            .unwrap();
        turn.node_id
    }

    async fn continuation(&mut self) -> String {
        self.mode("normal");
        let one = self.complete(self.first, "Make a change").await;
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(self.root.join(format!(
                "seats/chat-fixture/chat-{}/mcp-assistant.json",
                self.first
            )))
            .unwrap(),
        )
        .unwrap();
        let servers = &config["mcpServers"];
        assert!(
            servers
                .as_object()
                .unwrap()
                .values()
                .all(|server| server["lifecycle"] == "lazy-keep-alive"),
            "chat servers must not start before scoped CLI flags bind: {servers}"
        );
        assert_eq!(
            servers["ai-team-planner"]["command"],
            serde_json::json!(std::env::current_exe().unwrap())
        );
        assert_eq!(
            servers["ai-team-planner"]["args"],
            serde_json::json!([
                "plan",
                "serve",
                "--db",
                self.db.to_str().unwrap(),
                "--chat",
                self.first.to_string(),
                "--node",
                one.to_string()
            ])
        );
        assert!(servers.get("ai-planner").is_none());
        assert!(servers.get("planner-alias").is_none());
        assert_eq!(servers["file-sql"]["command"], "file-sql");
        assert_eq!(servers["chrome-devtools"]["command"], "fixture-browser");
        assert!(
            !self.store.planning_path().unwrap().exists(),
            "ordinary turns need no plan database"
        );
        let session = self.store.node_run(one).unwrap().session_id.unwrap();
        assert_eq!(self.store.node_run(one).unwrap().status, NodeStatus::Done);
        let two = self.complete(self.first, "Now continue").await;
        assert_eq!(
            self.store.node_run(two).unwrap().session_id.as_deref(),
            Some(session.as_str())
        );
        assert_eq!(self.store.node_run(one).unwrap().usage.tokens_out, 20);
        assert_eq!(self.store.node_run(two).unwrap().usage.tokens_out, 20);
        let reloaded = Store::open(&self.db).unwrap();
        assert_eq!(reloaded.chat_turns(self.first).unwrap().len(), 2);
        assert!(reloaded.chat_events(self.other, 0, 500).unwrap().is_empty());
        session
    }

    async fn long_running(&mut self) {
        self.mode("long-running");
        let node_id = self
            .complete(
                self.first,
                "Ground, investigate and plan a substantial feature",
            )
            .await;
        let node = self.store.node_run(node_id).unwrap();
        assert_eq!(node.turns, 65);
        assert_eq!(node.status, NodeStatus::Done, "{:?}", node.blocked_reason);
        assert!(self
            .store
            .chat(self.first)
            .unwrap()
            .active_node_id
            .is_none());
    }

    async fn cancellation(&mut self) {
        self.mode("slow");
        let slow = self.submit(self.first, "Run a slow check");
        let worker_db = self.db.clone();
        let first = self.first;
        let task =
            tokio::spawn(async move { drive_chat(&worker_db, first, slow.node_id, false).await });
        until(|| {
            !self.store.chat(first).unwrap().live_text.is_empty()
                && self
                    .store
                    .chat_events(first, 0, 500)
                    .unwrap()
                    .iter()
                    .any(|event| event.summary == "bash")
        })
        .await;
        assert!(self
            .store
            .begin_chat_turn(
                self.other,
                "Competing writer",
                "busy",
                &ModelRegistry::load().unwrap()
            )
            .is_err());
        assert!(
            self.store.claim_chat_resume(first, slow.node_id).is_err(),
            "cannot resume a running child"
        );
        self.store.request_chat_stop(first, slow.node_id).unwrap();
        tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            self.store.node_run(slow.node_id).unwrap().status,
            NodeStatus::Cancelled
        );
        assert_eq!(self.store.chat(first).unwrap().active_node_id, None);
        assert!(
            self.repo.join("changed.txt").exists(),
            "stop must not return/reset the checkout"
        );
        let descendant = std::fs::read_to_string(self.root.join("tool.pid")).unwrap();
        until(|| {
            let output = Command::new("ps")
                .args(["-o", "stat=", "-p", descendant.trim()])
                .output()
                .unwrap();
            let status = String::from_utf8_lossy(&output.stdout);
            status.trim().is_empty() || status.trim().starts_with('Z')
        })
        .await;
    }

    async fn isolation_and_recovery(&mut self, session: &str) {
        self.mode("normal");
        let independent = self.complete(self.other, "Independent conversation").await;
        let independent = self.store.node_run(independent).unwrap();
        assert_ne!(independent.session_id.as_deref(), Some(session));
        assert!(self
            .store
            .chat_events(self.other, 0, 500)
            .unwrap()
            .iter()
            .all(|event| event.run_id == independent.run_id));
        // Also covers a crash before the pending user request reaches Pi.
        let interrupted = self.submit(self.first, "Recover this request");
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute(
            "UPDATE node_run SET supervisor_pid = NULL, pi_pid = NULL WHERE id = ?1",
            [interrupted.node_id],
        )
        .unwrap();
        assert_eq!(
            self.store
                .claim_chat_resume(self.first, interrupted.node_id)
                .unwrap(),
            interrupted.node_id
        );
        assert!(
            self.store
                .claim_chat_resume(self.first, interrupted.node_id)
                .is_err(),
            "recovery must be claimed once"
        );
        drive_chat(&self.db, self.first, interrupted.node_id, true)
            .await
            .unwrap();
        assert_eq!(
            self.store
                .node_run(interrupted.node_id)
                .unwrap()
                .session_id
                .as_deref(),
            Some(session)
        );
        let prompts = std::fs::read_to_string(self.root.join("prompts")).unwrap();
        assert!(prompts.contains("Now continue"));
        assert!(prompts.contains("The previous turn was interrupted"));
        assert!(prompts.contains("Last user request:\nRecover this request"));
    }

    async fn normal_exit_cleans_tools(&mut self) {
        for mode in ["orphan", "inherited-pipes"] {
            self.mode(mode);
            tokio::time::timeout(Duration::from_secs(5), self.complete(self.first, mode))
                .await
                .expect("a tool kept the exited parent's pipes open");
            let pid = std::fs::read_to_string(self.root.join("orphan.pid")).unwrap();
            until(|| {
                let output = Command::new("ps")
                    .args(["-o", "stat=", "-p", pid.trim()])
                    .output()
                    .unwrap();
                let status = String::from_utf8_lossy(&output.stdout);
                status.trim().is_empty() || status.trim().starts_with('Z')
            })
            .await;
            std::fs::remove_file(self.root.join("orphan.pid")).unwrap();
        }
    }

    async fn failed_outcomes(&mut self) {
        self.mode("fail");
        let failure = self.complete(self.first, "Report failure truthfully").await;
        assert_eq!(
            self.store.node_run(failure).unwrap().status,
            NodeStatus::Failed
        );
        assert!(self
            .store
            .chat(self.first)
            .unwrap()
            .active_node_id
            .is_none());
        self.mode("exitfail");
        let bad_exit = self.complete(self.first, "Exit failed").await;
        assert_eq!(
            self.store.node_run(bad_exit).unwrap().status,
            NodeStatus::Failed,
            "settled is not a pass when the process exits unsuccessfully"
        );
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // A failing assertion must not leave fake tool processes on the developer's Mac.
        if let Ok(group) = std::fs::read_to_string(self.root.join("orphan.group")) {
            if let Some(pid) = group
                .trim()
                .parse()
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        }
        for id in [self.first, self.other] {
            if let Ok(turns) = self.store.chat_turns(id) {
                for turn in turns {
                    if let Some(pid) = turn
                        .node
                        .pi_pid
                        .and_then(|pid| i32::try_from(pid).ok())
                        .and_then(rustix::process::Pid::from_raw)
                    {
                        let _ =
                            rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                    }
                }
            }
        }
    }
}

// This integration binary has one test. Its isolated environment cannot race another
// test in this process or reach an operator's configured planner, Pi or ai-team store.
#[tokio::test]
async fn conversations_continue_stop_recover_and_keep_other_chats_and_standalone_planning_untouched(
) {
    let mut harness = Harness::new();
    let models = ModelRegistry::load().unwrap().models().unwrap();
    assert_eq!(models[0].model, "fixture-model");
    let config = std::fs::read_to_string(harness.root.join("catalogue-config")).unwrap();
    assert!(
        !Path::new(&config).exists(),
        "catalogue config should be temporary"
    );
    let session = harness.continuation().await;
    harness.long_running().await;
    harness.cancellation().await;
    harness.isolation_and_recovery(&session).await;
    harness.failed_outcomes().await;
    harness.normal_exit_cleans_tools().await;
    assert!(!harness.root.join("planner-touched").exists());
    assert!(!harness.root.join("lease-touched").exists());
    assert!(
        !harness.root.join("team-home").exists(),
        "an explicitly opened DB owns its support files too"
    );
    assert!(std::fs::read_to_string(harness.root.join("calls"))
        .unwrap()
        .lines()
        .all(|call| call.contains("|llama.cpp|fixture-model")));
}
