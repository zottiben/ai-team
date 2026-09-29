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
    assert!(!env.path().join("unexpected-aip").exists());
    assert!(!env.path().join("unexpected-pi").exists());
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
    assert!(f.lease().worktree_path.is_some());
    assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_none());
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
    f.no_return();
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
