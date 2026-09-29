//! Real Git worktrees with a bounded fake awt transport. No models, no operator state.
#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use ai_team_core::{
    planning::{ChatPlan, PlanAccess, PlanAction, PlanActor, PlanStatus},
    *,
};
use tempfile::TempDir;
#[path = "chat_build/recovery.rs"]
mod recovery_tests;
use recovery_tests::{
    abandoned_tasks_lose_their_lock_but_do_not_certify_cleanup,
    recover_publication_return_and_acquisition_evidence,
    recovery_does_not_invent_quiescence_or_restart_a_model,
};

struct Fixture {
    dir: TempDir,
    repo: PathBuf,
    store: Store,
    chat: Chat,
    turn: ChatSubmission,
    registry: ModelRegistry,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("README.md"), "approved base\n").unwrap();
        git(&repo, &["add", "."]);
        commit(&repo);
        let mut store = Store::init(&dir.path().join("team.sqlite")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Build".into(),
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
        store.seed_default_team(project.id).unwrap();
        let chat = store
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
            .unwrap();
        let registry = ModelRegistry::local_only();
        let turn = store
            .begin_chat_turn(chat.id, "request", "Build the reviewed change", &registry)
            .unwrap();
        store
            .set_node_status(turn.node_id, NodeStatus::Done)
            .unwrap();
        // A review-pause fixture, not a claim that the model planned this work.
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute("UPDATE chat_team_run SET phase = 'awaiting_approval', supervisor_pid = NULL, supervisor_identity = NULL, rev = rev + 1 WHERE run_id = ?1", [turn.run_id]).unwrap();
        store
            .set_run_status(turn.run_id, RunStatus::Blocked)
            .unwrap();
        let mut f = Self {
            dir,
            repo,
            store,
            chat,
            turn,
            registry,
        };
        f.change(|expect_revision| PlanAction::CreatePlan {
            expect_revision,
            title: "Reviewed work".into(),
            summary: Some("Keep the solo checkout intact".into()),
        });
        f.add_slice("S1", vec!["crates/**"]);
        f
    }

    fn plan(&self) -> ChatPlan {
        self.store
            .chat_plan(self.chat.id, PlanActor::Human)
            .unwrap()
    }
    fn change(&mut self, action: impl FnOnce(i64) -> PlanAction) -> ChatPlan {
        let revision = self.plan().revision;
        self.store
            .change_chat_plan(self.chat.id, PlanActor::Human, action(revision))
            .unwrap()
    }
    fn add_slice(&mut self, key: &str, paths: Vec<&str>) {
        self.change(|expect_revision| PlanAction::AddSlice {
            expect_revision,
            key: key.into(),
            title: key.into(),
            scope: "Implement the feature".into(),
            touches: paths.into_iter().map(String::from).collect(),
            demo: "Run the checks".into(),
        });
    }
    fn agent(&self, role: &str) -> Agent {
        let team = self.store.run(self.turn.run_id).unwrap().team_id.unwrap();
        self.store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == role)
            .unwrap()
    }
    async fn approval(&mut self) -> ChatBuildApproval {
        let review = self
            .store
            .chat_build_review(self.chat.id, self.turn.node_id)
            .await
            .unwrap();
        assert!(!review.roster.is_empty());
        ChatBuildApproval {
            expect_control_revision: review.execution.rev,
            expect_plan_revision: review.plan.revision,
            expect_roster_revision: review.roster_revision,
            expect_head: review.head,
        }
    }
    async fn approve(&mut self) -> ChatBuildStart {
        let approval = self.approval().await;
        self.store
            .approve_chat_build(self.chat.id, self.turn.node_id, &approval)
            .await
            .unwrap()
    }
    async fn rejected(&mut self, approval: &ChatBuildApproval, reason: &str) {
        let error = self
            .store
            .approve_chat_build(self.chat.id, self.turn.node_id, approval)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(self
            .store
            .chat_build_slices(self.turn.run_id)
            .unwrap()
            .is_empty());
    }
    fn conn(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.store.path()).unwrap()
    }
    fn activate_awt(&self, mode: &str) {
        std::env::set_var("BUILD_TEST_ROOT", self.dir.path());
        std::fs::write(self.dir.path().join("awt-mode"), mode).unwrap();
    }
    async fn prepare(&mut self, control: &ChatBuildControl) -> Result<ai_planner_core::Slice> {
        self.store
            .prepare_chat_build_slice(control, "S1", &self.registry)
            .await
    }
    fn lease(&self) -> ChatBuildSlice {
        self.store
            .chat_build_slices(self.turn.run_id)
            .unwrap()
            .remove(0)
    }
    fn no_return(&self) {
        assert!(!self.dir.path().join("awt-returned").exists());
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn commit(repo: &Path) {
    git(
        repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    );
}

#[tokio::test]
async fn approvals_and_journaled_leases_preserve_exact_scope_and_files() {
    // One test owns process-global fixture env. No sibling test sees this PATH/HOME.
    let env = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", env.path());
    std::env::set_var("XDG_CONFIG_HOME", env.path().join("config"));
    std::env::set_var("AI_TEAM_HOME", env.path().join("state"));
    let bin = env.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, body) in [
        ("awt", include_str!("fixtures/chat-build-awt.sh")),
        (
            "aip",
            "#!/bin/sh\ntouch \"$HOME/unexpected-aip\"; exit 98\n",
        ),
        ("pi", "#!/bin/sh\ntouch \"$HOME/unexpected-pi\"; exit 99\n"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    legacy_controls_cannot_reinterpret_a_chat_run().await;
    bare_directories_are_rejected_before_approval().await;
    review_conflicts_and_dirty_files().await;
    question_answers_are_not_approval().await;
    routing_rejects_unowned_mixed_and_ambiguous_work().await;
    approval_cancellation_and_single_start().await;
    approval_and_definition_writes_serialize().await;
    approval_rows_commit_atomically().await;
    stopped_denied_and_capped_preparations_do_not_spend().await;
    stop_during_acquisition_retains_the_returned_lease().await;
    leased_work_keeps_its_approval_and_exact_worker_scope().await;
    failed_preparations_retain_evidence_and_never_reset_files().await;
    a_partial_claim_commit_is_reconciled_without_a_second_lease().await;
    a_reused_seat_id_cannot_inherit_approval().await;
    install_worker_runtime(&bin);
    recovery_does_not_invent_quiescence_or_restart_a_model().await;
    parallel_workers_commit_and_return_without_touching_solo().await;
    repairs_inherit_only_the_makers_session().await;
    failed_work_is_retained_and_does_not_poison_siblings().await;
    cancelled_workers_and_gates_reap_their_groups().await;
    abandoned_tasks_lose_their_lock_but_do_not_certify_cleanup().await;
    recover_publication_return_and_acquisition_evidence().await;
    assert!(!env.path().join("unexpected-aip").exists());
    assert!(!env.path().join("unexpected-pi").exists());
}

async fn bare_directories_are_rejected_before_approval() {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.repo.join("ui/src")).unwrap();
    std::fs::write(f.repo.join("ui/src/app.ts"), "source\n").unwrap();
    git(&f.repo, &["add", "."]);
    commit(&f.repo);
    f.add_slice("S2", vec!["ui/src"]);
    let approval = f.approval().await;
    f.rejected(&approval, "ui/src/**").await;
}

async fn legacy_controls_cannot_reinterpret_a_chat_run() {
    let mut f = Fixture::new();
    let run = f.store.run(f.turn.run_id).unwrap();
    let orchestrator = Orchestrator {
        db_path: f.store.path().into(),
        project_dir: f.dir.path().join("support"),
        repo: f.repo.clone(),
        run_id: run.id,
        team_id: run.team_id.unwrap(),
        registry: f.registry.clone(),
        parallel_width: 2,
        planner: Planner::at(&f.repo).for_plan("chat-1"),
        worktrees: Worktrees::at(&f.repo),
    };
    let error = orchestrator
        .build_slices(&mut f.store, |_| {})
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("chat controls"),
        "legacy dispatcher reached beyond its boundary: {error}"
    );
    for error in [
        claim_plan_approval(&mut f.store, run.id).unwrap_err(),
        claim_session_reset(&mut f.store, run.id, f.turn.node_id).unwrap_err(),
        f.store
            .claim_node_supervision(f.turn.node_id, 12, None)
            .unwrap_err(),
    ] {
        assert!(error.to_string().contains("chat controls"));
    }
    assert!(continue_approved_run_at(f.store.path(), run.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("chat controls"));
    let review = f
        .store
        .open_review(
            f.chat.project_id,
            "team draft",
            Some(run.id),
            Some(f.turn.node_id),
            None,
        )
        .unwrap();
    assert!(pending_review(&f.store, review.id)
        .unwrap_err()
        .to_string()
        .contains("chat controls"));
    assert_eq!(f.store.run(run.id).unwrap().rev, run.rev);
}

async fn review_conflicts_and_dirty_files() {
    let mut f = Fixture::new();
    let approval = f.approval().await;
    std::fs::write(f.repo.join("keep.txt"), "human draft").unwrap();
    f.rejected(&approval, "dirty").await;
    assert_eq!(
        std::fs::read_to_string(f.repo.join("keep.txt")).unwrap(),
        "human draft"
    );
    std::fs::remove_file(f.repo.join("keep.txt")).unwrap();
    std::fs::write(f.repo.join("README.md"), "staged human draft").unwrap();
    git(&f.repo, &["add", "."]);
    f.rejected(&approval, "dirty").await;
    assert!(git(&f.repo, &["diff", "--cached"]).contains("staged human draft"));
    commit(&f.repo);
    f.rejected(&approval, "HEAD changed").await;
    let approval = f.approval().await;
    f.change(|expect_revision| PlanAction::AppendLog {
        expect_revision,
        body: "new review evidence".into(),
        slice: None,
    });
    f.rejected(&approval, "plan changed").await;
    let approval = f.approval().await;
    let maker = f.agent("backend");
    f.store
        .set_agent_model(maker.id, Provider::Local, "changed")
        .unwrap();
    f.rejected(&approval, "team changed").await;
    let approval = f.approval().await;
    let mut stale = approval.clone();
    stale.expect_control_revision -= 1;
    f.rejected(&stale, "reviewed approval pause").await;
    let other = f
        .store
        .create_chat(NewChat {
            project_id: f.chat.project_id,
            workspace: f.repo.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    assert!(f
        .store
        .approve_chat_build(other.id, f.turn.node_id, &approval)
        .await
        .is_err());
    f.store
        .request_chat_stop(f.chat.id, f.turn.node_id)
        .unwrap();
    f.rejected(&approval, "reviewed approval pause").await;
}

async fn question_answers_are_not_approval() {
    let mut f = Fixture::new();
    let plan = f.change(|expect_revision| PlanAction::OpenQuestion {
        expect_revision,
        body: "Use this scope?".into(),
        slice: None,
    });
    let id = plan.bundle.unwrap().questions[0].id;
    let approval = f.approval().await;
    f.rejected(&approval, "open questions").await;
    f.change(|expect_revision| PlanAction::AnswerQuestion {
        expect_revision,
        question_id: id,
        answer: "Yes".into(),
    });
    assert_eq!(
        f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().phase,
        ChatTeamPhase::AwaitingApproval
    );
    assert!(f.store.chat_build_slices(f.turn.run_id).unwrap().is_empty());
    f.rejected(&approval, "plan changed").await;
    f.approve().await;
    assert_eq!(f.lease().lease_state, "pending");
}

async fn routing_rejects_unowned_mixed_and_ambiguous_work() {
    for (paths, reason) in [
        (vec!["unknown/**"], "no maker owns"),
        (vec!["crates/**", "ui/**"], "multiple maker zones"),
    ] {
        let mut f = Fixture::new();
        f.add_slice("S2", paths);
        let approval = f.approval().await;
        f.rejected(&approval, reason).await;
    }
    let mut f = Fixture::new();
    let backend = f.agent("backend");
    let mut same = NewAgent::from(&backend);
    same.role = "another-maker".into();
    let extra = f.store.add_agent(backend.team_id, same).unwrap();
    let approval = f.approval().await;
    f.rejected(&approval, "ambiguous ownership").await;
    let mut nested = NewAgent::from(&extra);
    nested.zone = "crates/special/**".into();
    f.store.update_agent(extra.id, nested).unwrap();
    let approval = f.approval().await;
    f.rejected(&approval, "another maker's zone").await;
    f.store.set_agent_enabled(extra.id, false).unwrap();
    f.store
        .set_agent_enabled(f.agent("verifier").id, false)
        .unwrap();
    let approval = f.approval().await;
    f.rejected(&approval, "read-only verifier").await;
}

async fn approval_cancellation_and_single_start() {
    let mut f = Fixture::new();
    let start = f.approve().await;
    assert!(f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Human,
            PlanAction::UpdateSlice {
                expect_revision: f.plan().revision,
                key: "S1".into(),
                title: "Changed after approval".into(),
                scope: "Not approved".into(),
                touches: vec!["crates/**".into()],
                demo: "Checks".into(),
            }
        )
        .is_err());
    f.store.cancel_unstarted_chat_build(&start).unwrap();
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_none());
    assert_eq!(f.lease().lease_state, "released");
    assert!(f.store.claim_chat_build(&start).is_err());
    f.change(|expect_revision| PlanAction::UpdateSlice {
        expect_revision,
        key: "S1".into(),
        title: "Editable again".into(),
        scope: "A later build".into(),
        touches: vec!["crates/**".into()],
        demo: "Checks".into(),
    });
    let mut f = Fixture::new();
    let approval = f.approval().await;
    let start = f.approve().await;
    assert!(f
        .store
        .approve_chat_build(f.chat.id, f.turn.node_id, &approval)
        .await
        .is_err());
    f.store.claim_chat_build(&start).unwrap();
    assert!(f.store.claim_chat_build(&start).is_err());
    assert!(f.store.cancel_unstarted_chat_build(&start).is_err());
    assert!(f
        .store
        .stop_chat_team_planning(f.chat.id, f.turn.node_id, start.revision)
        .is_err());
}

async fn leased_work_keeps_its_approval_and_exact_worker_scope() {
    let mut f = Fixture::new();
    f.add_slice("S2", vec!["ui/**"]);
    f.change(|expect_revision| PlanAction::SetSliceStatus {
        expect_revision,
        key: "S2".into(),
        status: PlanStatus::Blocked,
        reason: Some("Depends on S1".into()),
    });
    let original = f.plan();
    let start = f.approve().await;
    let control = f.store.claim_chat_build(&start).unwrap();
    f.activate_awt("normal");
    let root_head = git(&f.repo, &["rev-parse", "HEAD"]);
    let root_branch = git(&f.repo, &["symbolic-ref", "HEAD"]);
    let slice = f.prepare(&control).await.unwrap();
    let lease = f.lease();
    assert_eq!(f.store.chat_build_slices(start.run_id).unwrap().len(), 1);
    assert_eq!(lease.lease_state, "leased");
    assert_eq!(lease.approved_rev, slice.rev - 1);
    assert_eq!(
        f.store
            .chat_team_run(start.run_id)
            .unwrap()
            .unwrap()
            .approved_revision,
        Some(original.revision)
    );
    let path = lease.worktree_path.as_deref().unwrap();
    assert_eq!(git(Path::new(path), &["rev-parse", "HEAD"]), root_head);
    assert_eq!(
        git(Path::new(path), &["symbolic-ref", "--short", "HEAD"]),
        lease.branch.as_deref().unwrap()
    );
    assert_eq!(git(&f.repo, &["symbolic-ref", "HEAD"]), root_branch);
    assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    assert!(f.prepare(&control).await.is_err());
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("awt-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(f
        .conn()
        .execute("UPDATE chat_build_slice SET approved_rev = 100", [])
        .is_err());
    assert!(f
        .conn()
        .execute("UPDATE chat_team_run SET base_sha = 'different'", [])
        .is_err());
    let revision = f.plan().revision;
    assert!(f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Human,
            PlanAction::SetSliceStatus {
                expect_revision: revision,
                key: "S1".into(),
                status: PlanStatus::Done,
                reason: None
            }
        )
        .is_err());
    assert!(f
        .store
        .dispatch(
            start.run_id,
            f.agent("frontend").id,
            Some("S1"),
            &f.registry
        )
        .is_err());
    let maker = f
        .store
        .dispatch(start.run_id, f.agent("backend").id, Some("S1"), &f.registry)
        .unwrap();
    f.store
        .attach_worktree(
            maker.id,
            path,
            lease.branch.as_deref(),
            lease.lease_holder.as_deref(),
        )
        .unwrap();
    f.store
        .set_node_status(maker.id, NodeStatus::Running)
        .unwrap();
    assert_eq!(
        f.store
            .planning_access(f.chat.id, PlanActor::Agent(maker.id))
            .unwrap(),
        PlanAccess::Maker
    );
    assert!(f
        .store
        .dispatch(start.run_id, f.agent("backend").id, Some("S1"), &f.registry)
        .is_err());
    let verifier = f.agent("verifier");
    assert!(f
        .store
        .dispatch(start.run_id, verifier.id, Some("S1"), &f.registry)
        .is_err());
    assert!(f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Agent(maker.id),
            PlanAction::SetSliceStatus {
                expect_revision: revision,
                key: "S1".into(),
                status: PlanStatus::Done,
                reason: None
            }
        )
        .is_err());
    f.store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Agent(maker.id),
            PlanAction::SetSliceStatus {
                expect_revision: revision,
                key: "S1".into(),
                status: PlanStatus::InReview,
                reason: None,
            },
        )
        .unwrap();
    assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_some());
    f.store.set_node_status(maker.id, NodeStatus::Done).unwrap();
    assert!(f
        .store
        .dispatch(start.run_id, verifier.id, None, &f.registry)
        .is_err());
    let reader = f
        .store
        .dispatch(start.run_id, verifier.id, Some("S1"), &f.registry)
        .unwrap();
    f.store
        .attach_worktree(reader.id, path, None, None)
        .unwrap();
    f.store
        .set_node_status(reader.id, NodeStatus::Running)
        .unwrap();
    assert_eq!(
        f.store
            .planning_access(f.chat.id, PlanActor::Agent(reader.id))
            .unwrap(),
        PlanAccess::Reader
    );
    f.no_return();
}

async fn failed_preparations_retain_evidence_and_never_reset_files() {
    for mode in [
        "dirty",
        "root",
        "foreign",
        "fail",
        "branch",
        "changed-source",
    ] {
        let mut f = Fixture::new();
        let start = f.approve().await;
        let control = f.store.claim_chat_build(&start).unwrap();
        let head = git(&f.repo, &["rev-parse", "HEAD"]);
        f.activate_awt(if mode == "branch" { "normal" } else { mode });
        if mode == "foreign" {
            std::fs::create_dir(f.dir.path().join("foreign")).unwrap();
        }
        if mode == "branch" {
            git(&f.repo, &["branch", f.lease().branch.as_deref().unwrap()]);
        }
        assert!(f.prepare(&control).await.is_err(), "{mode}");
        let lease = f.lease();
        assert_eq!(lease.lease_state, "retained", "{mode}");
        assert!(lease.reason.is_some());
        assert!(lease.lease_holder.is_some());
        assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), head);
        assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_none());
        assert!(f.store.claim_chat_build_slice(&control, "S1").is_err());
        if mode == "dirty" {
            assert_eq!(
                std::fs::read_to_string(f.dir.path().join("lease-1/keep.txt")).unwrap(),
                "keep this file\n"
            );
        }
        if mode == "changed-source" {
            assert_eq!(
                std::fs::read_to_string(f.repo.join("later.txt")).unwrap(),
                "new human draft\n"
            );
        }
        if mode == "fail" {
            assert!(f.dir.path().join("lease-1/.git").exists());
            assert!(lease.worktree_path.is_none());
        }
        assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
        f.no_return();
    }
    let mut f = Fixture::new();
    let start = f.approve().await;
    let control = f.store.claim_chat_build(&start).unwrap();
    f.activate_awt("normal");
    std::fs::write(f.repo.join("later.txt"), "post-approval work").unwrap();
    assert!(f
        .prepare(&control)
        .await
        .unwrap_err()
        .to_string()
        .contains("checkout changed"));
    assert!(!f.dir.path().join("awt-calls").exists());
    assert_eq!(f.lease().lease_state, "pending");
}

async fn a_partial_claim_commit_is_reconciled_without_a_second_lease() {
    let mut f = Fixture::new();
    let start = f.approve().await;
    let control = f.store.claim_chat_build(&start).unwrap();
    f.activate_awt("normal");
    f.conn().execute_batch("CREATE TRIGGER fail_claim BEFORE UPDATE OF lease_state ON chat_build_slice WHEN NEW.lease_state = 'leased' BEGIN SELECT RAISE(ABORT, 'injected team commit failure'); END;").unwrap();
    let error = f.prepare(&control).await.unwrap_err();
    assert!(
        error.to_string().contains("injected team commit failure"),
        "{error}"
    );
    assert_eq!(f.lease().lease_state, "retained");
    assert!(
        f.store
            .dispatch(start.run_id, f.agent("backend").id, Some("S1"), &f.registry)
            .is_err(),
        "an engine claim alone must not authorize a worker"
    );
    let claimed = f.plan().bundle.unwrap().slices.remove(0);
    assert_eq!(claimed.rev, f.lease().approved_rev + 1);
    assert!(claimed.claimed_by.is_some());
    f.conn().execute_batch("DROP TRIGGER fail_claim;").unwrap();
    let reconciled = f.store.claim_chat_build_slice(&control, "S1").unwrap();
    assert_eq!(reconciled.rev, claimed.rev);
    assert_eq!(f.lease().lease_state, "leased");
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("awt-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let revision = f.plan().revision;
    f.store.claim_chat_build_slice(&control, "S1").unwrap();
    assert_eq!(f.plan().revision, revision);
    f.conn()
        .execute("UPDATE chat_team_run SET rev = rev + 1", [])
        .unwrap();
    assert!(f.store.claim_chat_build_slice(&control, "S1").is_err());
    f.no_return();
}

async fn approval_and_definition_writes_serialize() {
    for _ in 0..3 {
        let mut f = Fixture::new();
        let approval = f.approval().await;
        let revision = approval.expect_plan_revision;
        let path = f.store.path().to_owned();
        let chat = f.chat.id;
        let node = f.turn.node_id;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let ready = barrier.clone();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let mut store = Store::open(&writer_path).unwrap();
            ready.wait();
            store.change_chat_plan(
                chat,
                PlanActor::Human,
                PlanAction::UpdateSlice {
                    expect_revision: revision,
                    key: "S1".into(),
                    title: "New work".into(),
                    scope: "Not the reviewed work".into(),
                    touches: vec!["crates/**".into()],
                    demo: "Checks".into(),
                },
            )
        });
        let controller = std::thread::spawn(move || {
            let mut store = Store::open(&path).unwrap();
            barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(store.approve_chat_build(chat, node, &approval))
        });
        let written = writer.join().unwrap();
        let approved = controller.join().unwrap();
        assert_ne!(
            written.is_ok(),
            approved.is_ok(),
            "exactly one side of the review conflict can commit"
        );
        assert_eq!(
            !f.store.chat_build_slices(f.turn.run_id).unwrap().is_empty(),
            approved.is_ok()
        );
    }
}

async fn approval_rows_commit_atomically() {
    let mut f = Fixture::new();
    f.add_slice("S2", vec!["ui/**"]);
    f.conn().execute_batch("CREATE TRIGGER fail_approval BEFORE INSERT ON chat_build_slice WHEN NEW.slice_key = 'S2' BEGIN SELECT RAISE(ABORT, 'injected approval failure'); END;").unwrap();
    let approval = f.approval().await;
    f.rejected(&approval, "injected approval failure").await;
    let execution = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
    assert_eq!(execution.phase, ChatTeamPhase::AwaitingApproval);
    assert!(execution.approved_revision.is_none());
    assert_eq!(f.plan().revision, approval.expect_plan_revision);
    f.conn()
        .execute_batch("DROP TRIGGER fail_approval;")
        .unwrap();
    f.store
        .approve_chat_build(f.chat.id, f.turn.node_id, &approval)
        .await
        .unwrap();
    assert_eq!(f.store.chat_build_slices(f.turn.run_id).unwrap().len(), 2);
}

async fn stopped_denied_and_capped_preparations_do_not_spend() {
    for case in ["stop", "policy", "budget"] {
        let mut f = Fixture::new();
        let start = f.approve().await;
        let control = f.store.claim_chat_build(&start).unwrap();
        f.activate_awt("normal");
        match case {
            "stop" => {
                f.store
                    .request_chat_stop(f.chat.id, f.turn.node_id)
                    .unwrap();
            }
            "policy" => {
                f.registry =
                    ModelRegistry::new(MachineProfile::parse(DEFAULT_MACHINE_PROFILE).unwrap());
            }
            "budget" => {
                f.conn()
                    .execute("UPDATE run SET budget_tokens = 0", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(f.prepare(&control).await.is_err(), "{case}");
        assert!(!f.dir.path().join("awt-calls").exists());
        assert_eq!(f.lease().lease_state, "pending");
    }
}

async fn stop_during_acquisition_retains_the_returned_lease() {
    let mut f = Fixture::new();
    let start = f.approve().await;
    let control = f.store.claim_chat_build(&start).unwrap();
    f.activate_awt("pause");
    let path = f.store.path().to_owned();
    let worker = tokio::spawn(async move {
        Store::open(&path)
            .unwrap()
            .prepare_chat_build_slice(&control, "S1", &ModelRegistry::local_only())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !f.dir.path().join("awt-waiting").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.store
        .request_chat_stop(f.chat.id, f.turn.node_id)
        .unwrap();
    std::fs::write(f.dir.path().join("awt-release"), "resume fixture").unwrap();
    assert!(worker.await.unwrap().is_err());
    assert_eq!(f.lease().lease_state, "retained");
    // Stop may kill awt before it prints the path. The durable holder still identifies
    // the uncertain acquisition and the lease must never be guessed safe to return.
    assert!(f.lease().lease_holder.is_some());
    assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_none());
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
    f.no_return();
}

fn install_worker_runtime(bin: &Path) {
    let config = PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap()).join("ai-team");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("machine.toml"),
        DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
    )
    .unwrap();
    std::env::set_var("ANTHROPIC_API_KEY", "must-not-reach-children");
    std::env::set_var("OPENAI_API_KEY", "must-not-reach-children");
    for (name, body) in [
        ("awt", include_str!("fixtures/chat-worker-awt.mjs")),
        ("pi", include_str!("fixtures/chat-worker-pi.mjs")),
    ] {
        let script = bin.join(format!("{name}.mjs"));
        std::fs::write(&script, body).unwrap();
        std::fs::write(
            bin.join(name),
            format!("#!/bin/sh\nexec node '{}' \"$@\"\n", script.display()),
        )
        .unwrap();
    }
}

fn worker_fixture(mode: &str) -> Fixture {
    let f = Fixture::new();
    f.activate_awt("normal");
    std::fs::write(f.dir.path().join("worker-mode"), mode).unwrap();
    std::fs::write(
        f.repo.join("package.json"),
        r#"{"scripts":{"test":"node check.mjs"}}"#,
    )
    .unwrap();
    std::fs::write(
        f.repo.join("check.mjs"),
        include_str!("fixtures/chat-worker-gate.mjs"),
    )
    .unwrap();
    std::fs::create_dir_all(f.repo.join("crates/dist")).unwrap();
    std::fs::write(f.repo.join("crates/dist/base.txt"), "tracked output\n").unwrap();
    std::fs::write(f.repo.join("crates/old name-é.txt"), "original source\n").unwrap();
    git(&f.repo, &["add", "."]);
    commit(&f.repo);
    f
}

fn calls(f: &Fixture) -> Vec<serde_json::Value> {
    std::fs::read_to_string(f.dir.path().join("worker-calls"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn parallel_workers_commit_and_return_without_touching_solo() {
    let mut f = worker_fixture("success");
    f.add_slice("S2", vec!["crates/**"]);
    f.add_slice("S3", vec!["ui/**"]);
    let base = git(&f.repo, &["rev-parse", "HEAD"]);
    let source_branch = git(&f.repo, &["symbolic-ref", "HEAD"]);
    let start = f.approve().await;
    drive_chat_team_build(f.store.path(), start.clone())
        .await
        .unwrap();
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    let run = f.store.run(f.turn.run_id).unwrap();
    assert_eq!(run.status, RunStatus::Done);
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_none());
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&f.repo, &["symbolic-ref", "HEAD"]), source_branch);
    assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    let records = calls(&f);
    assert_eq!(records.len(), 6, "three makers and three scoped verifiers");
    let makers: Vec<_> = records
        .iter()
        .filter(|call| call["role"] != "verifier")
        .map(|call| call["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        makers.last(),
        Some(&"S2"),
        "busy backend work must be deferred, not dropped"
    );
    let configs: std::collections::HashSet<_> = records
        .iter()
        .map(|call| call["configPath"].as_str().unwrap())
        .collect();
    assert_eq!(configs.len(), 6);
    for slice in f.store.chat_build_slices(run.id).unwrap() {
        assert_eq!(slice.build_status, "verified");
        assert_eq!(slice.lease_state, "released");
        assert!(slice.release_started);
        let branch = slice.branch.unwrap();
        let sha = slice.commit_sha.unwrap();
        assert_eq!(git(&f.repo, &["rev-parse", &branch]), sha);
        assert_eq!(git(&f.repo, &["rev-parse", &format!("{sha}^")]), base);
        let files = git(
            &f.repo,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", &sha],
        );
        assert!(files.contains(&format!("{}.txt", slice.slice_key)));
        assert!(!files.contains("dist/"));
    }
    for slice in f.plan().bundle.unwrap().slices {
        assert_eq!(slice.status, PlanStatus::InReview);
        assert!(slice.claimed_by.is_none());
    }
    for column in ["candidate_sha", "commit_sha"] {
        assert!(f
            .conn()
            .execute(
                &format!("UPDATE chat_build_slice SET {column} = 'rewritten'"),
                []
            )
            .is_err());
    }
    let reviews = f.store.reviews(Some(f.chat.project_id), true).unwrap();
    assert_eq!(reviews.len(), 3);
    assert!(reviews
        .iter()
        .all(|review| review.base_sha.as_deref() == Some(&base) && review.head_sha.is_some()));
    assert!(f
        .store
        .chat_team_members(run.id)
        .unwrap()
        .iter()
        .all(|member| !member.pi_alive()));
    return_to_solo_keeps_delivery_evidence(&mut f).await;
}

async fn return_to_solo_keeps_delivery_evidence(f: &mut Fixture) {
    let chat = f.store.chat(f.chat.id).unwrap();
    f.store
        .set_chat_mode(chat.id, ChatMode::Single, chat.rev)
        .unwrap();
    let turn = f
        .store
        .begin_chat_turn(
            chat.id,
            "Review those draft branches",
            "solo-after-build",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    drive_chat(f.store.path(), chat.id, turn.node_id, false)
        .await
        .unwrap();
    let prompt =
        std::fs::read_to_string(f.dir.path().join(format!("worker-{}.prompt", turn.node_id)))
            .unwrap();
    assert!(prompt.contains("Recorded verified team drafts"));
    assert!(prompt.contains("NOT merged into this chat's solo checkout"));
    for slice in f.store.chat_build_slices(f.turn.run_id).unwrap() {
        assert!(prompt.contains(slice.commit_sha.as_deref().unwrap()));
        assert!(prompt.contains(slice.branch.as_deref().unwrap()));
    }
}

async fn repairs_inherit_only_the_makers_session() {
    let mut f = worker_fixture("repair");
    let start = f.approve().await;
    drive_chat_team_build(f.store.path(), start).await.unwrap();
    let records = calls(&f);
    let makers: Vec<_> = records
        .iter()
        .filter(|call| call["role"] == "backend")
        .collect();
    assert_eq!(makers.len(), 2);
    assert_ne!(makers[0]["id"], makers[1]["id"]);
    assert_eq!(makers[0]["session"], makers[1]["session"]);
    let reader = records
        .iter()
        .find(|call| call["role"] == "verifier")
        .unwrap();
    assert_ne!(reader["session"], makers[0]["session"]);
    let nodes: Vec<_> = f
        .store
        .node_runs(f.turn.run_id)
        .unwrap()
        .into_iter()
        .filter(|node| node.role == "backend")
        .collect();
    assert_eq!(nodes[0].status, NodeStatus::Failed);
    assert_eq!(nodes[1].attempt, nodes[0].attempt + 1);
    assert!(nodes[1].stream_cursor > nodes[0].stream_cursor);
    assert!(nodes.iter().all(|node| node.usage.tokens_out > 0));
    assert!(
        std::fs::read_to_string(f.dir.path().join(format!("worker-{}.prompt", nodes[1].id)))
            .unwrap()
            .contains("implementation is BAD")
    );
}

async fn failed_work_is_retained_and_does_not_poison_siblings() {
    for mode in [
        "tracked-output",
        "siblings",
        "noop",
        "outside",
        "reject",
        "echo-only",
        "exit-fail",
        "verifier-edit",
        "gate-edit",
        "rename",
        "orphan-gate",
        "settle-fail",
        "return-fail",
        "board-fail",
        "staged-only",
        "node-cap",
    ] {
        let mut f = worker_fixture(mode);
        f.conn()
            .execute("UPDATE run SET max_repairs = 0", [])
            .unwrap();
        if mode == "siblings" {
            f.add_slice("S3", vec!["ui/**"]);
        }
        if mode == "node-cap" {
            f.conn()
                .execute("UPDATE run SET budget_tokens_node = 1", [])
                .unwrap();
        }
        if mode == "settle-fail" {
            f.conn().execute_batch("CREATE TRIGGER reject_worker_settlement BEFORE UPDATE ON node_run WHEN NEW.role = 'backend' AND NEW.status = 'failed' BEGIN SELECT RAISE(ABORT, 'injected member settlement failure'); END;").unwrap();
        }
        if mode == "board-fail" {
            f.conn().execute_batch("CREATE TRIGGER reject_board_event BEFORE INSERT ON event WHEN NEW.summary LIKE 'S1: draft ready for human review%' BEGIN SELECT RAISE(ABORT, 'injected board settlement failure'); END;").unwrap();
        }
        let start = f.approve().await;
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            drive_chat_team_build(f.store.path(), start),
        )
        .await
        .unwrap();
        if matches!(mode, "rename" | "orphan-gate" | "tracked-output") {
            outcome.unwrap();
            if mode == "rename" {
                let sha = f.lease().commit_sha.unwrap();
                let files = git(&f.repo, &["ls-tree", "-r", "--name-only", &sha]);
                assert!(files.contains("new name"));
                assert!(!files.contains("old name"));
            } else if mode == "tracked-output" {
                let sha = f.lease().commit_sha.unwrap();
                assert!(git(&f.repo, &["ls-tree", "-r", "--name-only", &sha]).contains("crates/dist/new.txt"), "new files in committed generated directories are source, not disposable gate output");
            } else {
                assert_process_dead(&f.dir.path().join("gate-tool.pid")).await;
            }
            continue;
        }
        assert!(outcome.is_err(), "{mode} must not be reported as verified");
        if mode == "settle-fail" {
            let error = outcome.unwrap_err().to_string();
            assert!(
                error.contains("Implemented S1."),
                "original worker failure disappeared: {error}"
            );
            assert!(error.contains("injected member settlement failure"));
            assert_eq!(f.lease().lease_state, "retained");
            continue;
        }
        if matches!(mode, "return-fail" | "board-fail") {
            assert_eq!(f.lease().build_status, "verified");
            assert!(f.lease().release_started && f.lease().commit_sha.is_some());
            let returned = mode == "board-fail";
            assert_eq!(
                f.lease().lease_state,
                if returned { "released" } else { "retained" }
            );
            let board = f.plan().bundle.unwrap().slices.remove(0);
            assert_eq!(board.status, PlanStatus::InReview);
            assert_eq!(
                board.claimed_by.is_none(),
                returned,
                "a failed return must retain its planner claim"
            );
            assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
            continue;
        }
        if mode == "staged-only" {
            assert!(git(
                Path::new(f.lease().worktree_path.as_deref().unwrap()),
                &["show", ":crates/staged-only.txt"]
            )
            .contains("preserve staged-only work"));
        }
        if mode == "node-cap" {
            assert!(calls(&f).iter().all(|call| call["role"] != "verifier"));
            assert!(!f.dir.path().join("gate-calls").exists());
        }
        assert_eq!(f.lease().lease_state, "retained", "{mode}");
        assert_eq!(f.lease().build_status, "failed", "{mode}");
        assert!(f.lease().commit_sha.is_none());
        assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
        assert_eq!(
            f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().phase,
            ChatTeamPhase::Blocked
        );
        assert!(Path::new(f.lease().worktree_path.as_deref().unwrap()).is_dir());
        if mode == "siblings" {
            let other = f.store.chat_build_slices(f.turn.run_id).unwrap().remove(1);
            assert_eq!(other.build_status, "verified");
            assert_eq!(other.lease_state, "released");
        }
        assert!(git(&f.repo, &["status", "--porcelain"]).is_empty());
    }
}

async fn cancelled_workers_and_gates_reap_their_groups() {
    for mode in ["slow-maker", "slow-gate"] {
        let mut f = worker_fixture(mode);
        let start = f.approve().await;
        let db = f.store.path().to_owned();
        let worker = tokio::spawn(async move { drive_chat_team_build(&db, start).await });
        let marker = if mode == "slow-maker" {
            "worker-waiting"
        } else {
            "gate-waiting"
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !f.dir.path().join(marker).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        f.store
            .request_chat_stop(f.chat.id, f.turn.node_id)
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(6), worker)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert_process_dead(&f.dir.path().join(if mode == "slow-maker" {
            "worker-tool.pid"
        } else {
            "gate-tool.pid"
        }))
        .await;
        assert_eq!(f.lease().lease_state, "retained");
        assert_eq!(f.lease().build_status, "stopped");
        assert!(
            !f.store
                .events(f.turn.run_id, None, 500)
                .unwrap()
                .iter()
                .any(|event| event.summary.starts_with("gate `")
                    && event
                        .payload
                        .as_ref()
                        .is_some_and(|payload| payload["passed"] == false)),
            "operator cancellation is not a failed project gate"
        );
        assert!(std::fs::read_to_string(
            Path::new(f.lease().worktree_path.as_deref().unwrap()).join("crates/S1.txt")
        )
        .unwrap()
        .contains("GOOD"));
        assert!(!std::fs::read_to_string(f.dir.path().join("pool-calls"))
            .unwrap()
            .contains("return"));
        assert!(f
            .store
            .chat_team_members(f.turn.run_id)
            .unwrap()
            .iter()
            .all(|member| !member.pi_alive()));
    }
}

async fn assert_process_dead(file: &Path) {
    let pid = std::fs::read_to_string(file).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let output = Command::new("ps")
                .args(["-p", pid.trim(), "-o", "stat="])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || state.trim().is_empty() || state.trim().starts_with('Z')
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

async fn a_reused_seat_id_cannot_inherit_approval() {
    let mut f = Fixture::new();
    let backend = f.agent("backend");
    f.store.set_agent_enabled(backend.id, false).unwrap();
    let mut config = NewAgent::from(&backend);
    config.role = "worker".into();
    let original = f.store.add_agent(backend.team_id, config.clone()).unwrap();
    let start = f.approve().await;
    let control = f.store.claim_chat_build(&start).unwrap();
    f.activate_awt("normal");
    f.store.delete_agent(original.id).unwrap();
    config.prompt_preset = None;
    config.prompt_md = Some("Different instructions, same rowid and revision".into());
    let replacement = f.store.add_agent(backend.team_id, config).unwrap();
    assert_eq!(original.id, replacement.id);
    assert_eq!(original.rev, replacement.rev);
    assert!(f
        .prepare(&control)
        .await
        .unwrap_err()
        .to_string()
        .contains("maker changed"));
    assert!(!f.dir.path().join("awt-calls").exists());
}
