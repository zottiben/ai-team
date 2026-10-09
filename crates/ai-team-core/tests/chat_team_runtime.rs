//! Real Pi-shaped children and controller persistence, no model/subscription calls.
//! Scoped planner writes are simulated through the shared service, not a fake MCP server.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use ai_team_core::planning::{PlanAccess, PlanAction, PlanActor};
use ai_team_core::{
    drive_chat_team_planning, ChatMode, ChatSubmission, ChatTeamPhase, ModelRegistry, NewChat,
    NewProject, NodeStatus, Provider, Reasoning, RunStatus, Store,
};

struct Harness {
    dir: tempfile::TempDir,
    db: PathBuf,
    repo: PathBuf,
    store: Store,
    chat: i64,
    other: i64,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let bin = root.join("bin");
        let repo = root.join("repo");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(root.join("config/ai-team")).unwrap();
        std::fs::write(
            root.join("config/ai-team/machine.toml"),
            ai_team_core::DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
        )
        .unwrap();
        for (name, body) in [
            ("pi", include_str!("fixtures/chat-team-pi.sh")),
            (
                "aip",
                "#!/bin/sh\ntouch \"$TEAM_TEST_ROOT/aip-called\"\nexit 99\n",
            ),
            (
                "awt",
                "#!/bin/sh\ntouch \"$TEAM_TEST_ROOT/awt-called\"\nexit 99\n",
            ),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("AI_TEAM_HOME", root.join("unused-home"));
        std::env::set_var("TEAM_TEST_ROOT", &root);
        std::env::set_var("ANTHROPIC_API_KEY", "must-not-reach-pi");
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        std::fs::write(repo.join("dirty.txt"), "keep my uncommitted solo work\n").unwrap();
        std::fs::write(
            repo.join("AGENTS.md"),
            "Repository rule: keep the handmade fixtures.",
        )
        .unwrap();
        std::fs::write(repo.join(".mcp.json"), r#"{"mcpServers":{"ai-team-planner":{"command":"shadow"},"legacy":{"command":"/usr/bin/aip"},"file-sql":{"command":"file-sql"},"chrome-devtools":{"command":"browser"}}}"#).unwrap();
        let db = root.join("team.db");
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Team runtime".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())
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
        let mut new_chat = || {
            store
                .create_chat_in_mode(
                    NewChat {
                        project_id: project.id,
                        workspace: repo.clone(),
                        provider: Provider::Local,
                        model: "fixture".into(),
                        reasoning: Reasoning::High,
                    },
                    ChatMode::Team,
                )
                .unwrap()
                .id
        };
        let chat = new_chat();
        let other = new_chat();
        Self {
            dir,
            db,
            repo,
            store,
            chat,
            other,
        }
    }

    fn submit(&mut self, label: &str) -> ChatSubmission {
        self.store
            .begin_chat_turn(self.chat, label, label, &ModelRegistry::load().unwrap())
            .unwrap()
    }

    fn mode(&self, value: &str) {
        std::fs::write(self.dir.path().join("mode"), value).unwrap();
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn spawn(&self, node: i64) -> tokio::task::JoinHandle<ai_team_core::Result<()>> {
        let db = self.db.clone();
        let chat = self.chat;
        tokio::spawn(async move { drive_chat_team_planning(&db, chat, node).await })
    }

    fn seed_solo_history(&mut self) {
        let chat = self.store.chat(self.chat).unwrap();
        self.store
            .set_chat_mode(chat.id, ChatMode::Single, chat.rev)
            .unwrap();
        let solo = self.submit("Solo requirement: preserve the existing cache");
        self.store
            .set_node_session(solo.node_id, "solo-before-team")
            .unwrap();
        self.store.ingest_pi_events(solo.node_id, "solo-before-team", 0, &[
            ai_team_core::PiEvent::parse(r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"We can reuse that cache."}]}}"#).unwrap(),
            ai_team_core::PiEvent::parse(r#"{"type":"agent_settled"}"#).unwrap(),
        ]).unwrap();
        self.store
            .finish_chat_turn(self.chat, solo.node_id, NodeStatus::Done, None)
            .unwrap();
        let chat = self.store.chat(self.chat).unwrap();
        self.store
            .set_chat_mode(chat.id, ChatMode::Team, chat.rev)
            .unwrap();
    }

    async fn return_to_solo(&mut self) {
        let chat = self.store.chat(self.chat).unwrap();
        self.store
            .set_chat_mode(chat.id, ChatMode::Single, chat.rev)
            .unwrap();
        let solo = self.submit("Continue solo after team review");
        ai_team_core::drive_chat(&self.db, self.chat, solo.node_id, false)
            .await
            .unwrap();
        assert_eq!(
            self.store
                .node_run(solo.node_id)
                .unwrap()
                .session_id
                .as_deref(),
            Some("solo-before-team")
        );
        let prompt =
            std::fs::read_to_string(self.dir.path().join(format!("chat-{}.prompt", self.chat)))
                .unwrap();
        assert!(prompt.contains("Grounded brief; plan ready for human review."));
        assert!(!prompt.contains("Solo requirement: preserve the existing cache"));
        let chat = self.store.chat(self.chat).unwrap();
        self.store
            .set_chat_mode(chat.id, ChatMode::Team, chat.rev)
            .unwrap();
    }

    async fn plan_and_pause(&mut self) {
        self.mode("normal");
        let turn = self.submit("Plan without losing my files");
        let task = self.spawn(turn.node_id);
        until(|| {
            self.store
                .chat_team_members(turn.run_id)
                .unwrap()
                .iter()
                .any(|member| member.node.role == "planner" && member.node.session_id.is_some())
        })
        .await;
        let planner = self
            .store
            .chat_team_members(turn.run_id)
            .unwrap()
            .into_iter()
            .find(|member| member.node.role == "planner")
            .unwrap()
            .node;
        assert_eq!(
            self.store.node_run(turn.node_id).unwrap().status,
            NodeStatus::Done
        );
        assert_eq!(
            self.store
                .planning_access(self.chat, PlanActor::Agent(planner.id))
                .unwrap(),
            PlanAccess::Planner
        );
        assert!(
            drive_chat_team_planning(&self.db, self.chat, turn.node_id)
                .await
                .is_err(),
            "a duplicate controller must not start another Pi"
        );
        assert!(drive_chat_team_planning(&self.db, self.other, turn.node_id)
            .await
            .is_err());
        let plan = self
            .store
            .change_chat_plan(
                self.chat,
                PlanActor::Agent(planner.id),
                PlanAction::CreatePlan {
                    expect_revision: 0,
                    title: "Owned team plan".into(),
                    summary: None,
                },
            )
            .unwrap();
        self.store
            .change_chat_plan(
                self.chat,
                PlanActor::Agent(planner.id),
                PlanAction::AddSlice {
                    expect_revision: plan.revision,
                    key: "S1".into(),
                    title: "One change".into(),
                    scope: "Implement it".into(),
                    touches: vec!["src/".into()],
                    demo: "Check the result".into(),
                },
            )
            .unwrap();
        std::fs::write(self.dir.path().join("plan-ready"), "ready").unwrap();
        task.await.unwrap().unwrap();
        let revision = self.store.chat_team_run(turn.run_id).unwrap().unwrap().rev;
        assert_eq!(
            ai_team_core::recover_abandoned_chat_teams(&self.db)
                .await
                .unwrap()[0]
                .state,
            ai_team_core::ChatRecoveryState::Quiescent
        );
        assert_eq!(
            self.store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
            revision
        );
        self.check_pause(&turn, planner.id);
        let prompt = std::fs::read_to_string(
            self.dir
                .path()
                .join(format!("node-{}.prompt", turn.node_id)),
        )
        .unwrap();
        assert!(prompt.contains("Solo requirement: preserve the existing cache"));
    }

    fn check_pause(&mut self, turn: &ChatSubmission, planner: i64) {
        let execution = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
        assert_eq!(execution.phase, ChatTeamPhase::AwaitingApproval);
        assert!(!execution.supervisor_alive());
        assert_eq!(
            self.store.run(turn.run_id).unwrap().status,
            RunStatus::Blocked
        );
        assert_eq!(
            self.store.chat(self.chat).unwrap().active_node_id,
            Some(turn.node_id)
        );
        assert!(self
            .store
            .chat_team_members(turn.run_id)
            .unwrap()
            .iter()
            .all(|member| member.node.status == NodeStatus::Done && member.node.pi_pid.is_none()));
        assert!(self
            .store
            .planning_access(self.chat, PlanActor::Agent(planner))
            .is_err());
        assert!(self
            .store
            .begin_chat_turn(
                self.other,
                "steal checkout",
                "steal",
                &ModelRegistry::load().unwrap()
            )
            .is_err());
        assert!(self
            .store
            .chat_plan(self.other, PlanActor::Human)
            .unwrap()
            .bundle
            .is_none());
        self.assert_configs(turn.node_id, planner);
        assert_eq!(
            Store::open(&self.db)
                .unwrap()
                .chat_team_run(turn.run_id)
                .unwrap()
                .unwrap()
                .phase,
            ChatTeamPhase::AwaitingApproval
        );
        assert!(self
            .store
            .stop_chat_team_planning(self.chat, turn.node_id, execution.rev - 1)
            .is_err());
        self.store
            .stop_chat_team_planning(self.chat, turn.node_id, execution.rev)
            .unwrap();
        assert!(self.store.chat(self.chat).unwrap().active_node_id.is_none());
        assert_eq!(self.calls(), 2);
    }

    fn assert_configs(&self, coordinator: i64, planner: i64) {
        for (node, access) in [
            (coordinator, PlanAccess::Coordinator),
            (planner, PlanAccess::Planner),
        ] {
            let path = self.dir.path().join(format!(
                "seats/team-runtime/chat-{}/node-{node}/mcp.json",
                self.chat
            ));
            let config: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let servers = &config["mcpServers"];
            assert!(
                servers
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|server| server["lifecycle"] == "lazy-keep-alive"),
                "team MCP must wait for scoped flags: {servers}"
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
                    self.db,
                    "--chat",
                    self.chat.to_string(),
                    "--node",
                    node.to_string()
                ])
            );
            assert_eq!(
                servers["ai-team-planner"]["includeTools"],
                serde_json::json!(access.tools())
            );
            assert!(servers.get("legacy").is_none());
            assert!(servers.get("chrome-devtools").is_none());
            assert!(servers.get("file-sql").is_some());
            let prompt =
                std::fs::read_to_string(self.dir.path().join(format!("node-{node}.prompt")))
                    .unwrap();
            assert!(prompt.contains("persistent checkout"));
            assert!(prompt.contains("Repository rule: keep the handmade fixtures."));
            assert!(!prompt.contains("`list_slices`"));
        }
    }

    async fn cancel(&mut self, role: &str) {
        self.mode(&format!("slow-{role}"));
        let turn = self.submit(&format!("Stop {role}"));
        let task = self.spawn(turn.node_id);
        until(|| {
            self.store
                .chat_team_members(turn.run_id)
                .unwrap()
                .iter()
                .any(|member| member.node.role == role && member.node.session_id.is_some())
                && self.dir.path().join("tool.pid").exists()
        })
        .await;
        self.store
            .request_chat_stop(self.chat, turn.node_id)
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            self.store.run(turn.run_id).unwrap().status,
            RunStatus::Cancelled
        );
        assert!(self.store.chat(self.chat).unwrap().active_node_id.is_none());
        let pid = std::fs::read_to_string(self.dir.path().join("tool.pid")).unwrap();
        until(|| dead(pid.trim())).await;
        std::fs::remove_file(self.dir.path().join("tool.pid")).unwrap();
    }

    async fn aborted_planning_task_is_recoverable(&mut self, role: &str) {
        self.mode(&format!("slow-{role}"));
        let turn = self.submit(&format!("Abort {role}"));
        let task = self.spawn(turn.node_id);
        until(|| {
            self.dir.path().join("tool.pid").exists()
                && self
                    .store
                    .chat_team_members(turn.run_id)
                    .unwrap()
                    .iter()
                    .any(|member| {
                        member.node.role == role
                            && member.node.session_id.is_some()
                            && !member.live_text.is_empty()
                    })
        })
        .await;
        let before = self.calls();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        until(|| {
            !self
                .store
                .chat_team_run(turn.run_id)
                .unwrap()
                .unwrap()
                .supervisor_alive()
        })
        .await;
        let execution = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
        assert!(!execution.quiescent);
        let report = ai_team_core::recover_abandoned_chat_teams(&self.db)
            .await
            .unwrap();
        assert_eq!(report[0].state, ai_team_core::ChatRecoveryState::Recovered);
        assert_eq!(report[0].target.run_id, turn.run_id);
        let recovered = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
        assert!(recovered.quiescent && !recovered.supervisor_alive());
        assert_eq!(recovered.phase, ChatTeamPhase::Blocked);
        assert!(
            self.store.chat(self.chat).unwrap().live_text.is_empty(),
            "recovery left a stale live preview"
        );
        assert_eq!(self.calls(), before);
        assert!(self
            .store
            .chat_team_members(turn.run_id)
            .unwrap()
            .iter()
            .filter(|member| member.node.role == role)
            .all(|member| member.node.session_id.is_some() && !member.pi_alive()));
        let pid = std::fs::read_to_string(self.dir.path().join("tool.pid")).unwrap();
        until(|| dead(pid.trim())).await;
        self.store
            .stop_chat_team_planning(self.chat, turn.node_id, recovered.rev)
            .unwrap();
        assert!(self.store.chat(self.chat).unwrap().active_node_id.is_none());
        std::fs::remove_file(self.dir.path().join("tool.pid")).unwrap();
    }

    async fn policy_is_enforced_but_historical_budgets_are_not(&mut self) {
        self.mode("normal");
        let before = self.calls();
        let turn = self.submit("Policy changed after reservation");
        let profile = self.dir.path().join("config/ai-team/machine.toml");
        let enabled = std::fs::read_to_string(&profile).unwrap();
        std::fs::write(&profile, enabled.replace("local = true", "local = false")).unwrap();
        assert!(drive_chat_team_planning(&self.db, self.chat, turn.node_id)
            .await
            .is_err());
        assert_eq!(self.calls(), before, "denied policy must not launch Pi");
        std::fs::write(&profile, enabled).unwrap();
        let execution = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
        self.store
            .stop_chat_team_planning(self.chat, turn.node_id, execution.rev)
            .unwrap();

        let capped = self.submit("Historical budgets do not stop planning");
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .execute(
                "UPDATE run SET budget_tokens = 0, budget_tokens_node = 0,
                    budget_seconds = 0, budget_seconds_node = 0, max_turns_node = 40 WHERE id = ?1",
                [capped.run_id],
            )
            .unwrap();
        drive_chat_team_planning(&self.db, self.chat, capped.node_id)
            .await
            .unwrap();
        assert_eq!(
            self.calls(),
            before + 2,
            "coordinator and planner must both finish"
        );
        assert!(self.store.run_usage(capped.run_id).unwrap().billable() > 0);
        assert_eq!(
            self.store.run(capped.run_id).unwrap().budget_tokens,
            Some(0)
        );
        let execution = self.store.chat_team_run(capped.run_id).unwrap().unwrap();
        assert_eq!(execution.phase, ChatTeamPhase::AwaitingApproval);
        self.store
            .stop_chat_team_planning(self.chat, capped.node_id, execution.rev)
            .unwrap();
    }

    async fn missing_plan_is_not_an_approval_pause(&mut self) {
        self.chat = self.other;
        self.mode("no-plan");
        let turn = self.submit("Do not pretend a prose promise made a plan");
        let error = drive_chat_team_planning(&self.db, self.chat, turn.node_id)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("no reviewable slices"),
            "{error}"
        );
        let execution = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
        assert_eq!(execution.phase, ChatTeamPhase::Blocked);
        assert!(self
            .store
            .chat_plan(self.chat, PlanActor::Human)
            .unwrap()
            .bundle
            .is_none());
        self.store
            .stop_chat_team_planning(self.chat, turn.node_id, execution.rev)
            .unwrap();
    }

    async fn settlement_failure_keeps_the_original_context_error(&mut self) {
        self.mode("contextfail");
        let turn = self.submit("preserve both errors");
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_planning_settlement BEFORE UPDATE ON node_run WHEN NEW.status = 'failed' BEGIN SELECT RAISE(ABORT, 'injected planning settlement failure'); END;").unwrap();
        let error = drive_chat_team_planning(&self.db, self.chat, turn.node_id)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("CONTEXT_UNAVAILABLE:"),
            "original planning failure disappeared: {error}"
        );
        assert!(error.contains("injected planning settlement failure"));
        conn.execute_batch("DROP TRIGGER reject_planning_settlement")
            .unwrap();
    }

    async fn failures_block_without_false_approval(&mut self) {
        for (mode, calls) in [("contextfail", 1), ("empty", 1), ("plannerexitfail", 2)] {
            self.mode(mode);
            let before = self.calls();
            let turn = self.submit(mode);
            assert!(drive_chat_team_planning(&self.db, self.chat, turn.node_id)
                .await
                .is_err());
            let execution = self.store.chat_team_run(turn.run_id).unwrap().unwrap();
            assert_eq!(execution.phase, ChatTeamPhase::Blocked);
            assert!(!execution.supervisor_alive());
            assert_eq!(self.calls() - before, calls);
            self.store
                .stop_chat_team_planning(self.chat, turn.node_id, execution.rev)
                .unwrap();
        }
    }
}

fn dead(pid: &str) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&output.stdout);
    status.trim().is_empty() || status.trim().starts_with('Z')
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("team runtime did not reach the expected state");
}

impl Drop for Harness {
    fn drop(&mut self) {
        for entry in std::fs::read_dir(self.dir.path()).unwrap().flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "group") {
                if let Some(pid) = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .and_then(rustix::process::Pid::from_raw)
                {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
            }
        }
    }
}

// One test owns this process's environment; all children/state live under its tempdir.
#[tokio::test]
async fn team_planning_runs_real_children_then_pauses_and_stops_without_legacy_dispatch() {
    let mut h = Harness::new();
    h.seed_solo_history();
    // A reservation is durable before the controller task exists. Recovery must
    // defeat a late callback without relying on the still-live desktop PID.
    let turn = h.submit("Reserved but never dispatched");
    let execution = h.store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert!(
        !execution.supervisor_alive(),
        "an unclaimed reservation has no live controller task"
    );
    let target = ai_team_core::ChatBuildRecovery {
        chat_id: h.chat,
        run_id: turn.run_id,
        node_id: turn.node_id,
        expect_revision: execution.rev,
    };
    let recovered = ai_team_core::recover_chat_team_processes(&h.db, &target)
        .await
        .unwrap();
    assert!(recovered.quiescent);
    assert!(drive_chat_team_planning(&h.db, h.chat, turn.node_id)
        .await
        .is_err());
    assert_eq!(h.calls(), 0);
    h.store
        .stop_chat_team_planning(h.chat, turn.node_id, recovered.rev)
        .unwrap();
    h.plan_and_pause().await;
    h.return_to_solo().await;
    h.cancel("orchestrator").await;
    h.cancel("planner").await;
    h.aborted_planning_task_is_recoverable("orchestrator").await;
    h.aborted_planning_task_is_recoverable("planner").await;
    h.failures_block_without_false_approval().await;
    h.policy_is_enforced_but_historical_budgets_are_not().await;
    h.missing_plan_is_not_an_approval_pause().await;
    h.settlement_failure_keeps_the_original_context_error()
        .await;
    assert_eq!(
        std::fs::read_to_string(h.repo.join("dirty.txt")).unwrap(),
        "keep my uncommitted solo work\n"
    );
    for path in ["aip-called", "awt-called", "unused-home"] {
        assert!(!h.dir.path().join(path).exists(), "unexpected {path}");
    }
}
