//! Close execution, not the lease. All tools/state remain inside chat_build's fixture.
use super::*;

fn request(f: &Fixture) -> ChatBuildClose {
    let execution = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
    ChatBuildClose {
        target: ChatBuildRecovery {
            chat_id: f.chat.id,
            run_id: f.turn.run_id,
            node_id: f.turn.node_id,
            expect_revision: execution.rev,
        },
        expect_plan_revision: f.plan().revision,
        expect_slices: f
            .store
            .chat_build_slices(f.turn.run_id)
            .unwrap()
            .into_iter()
            .map(|s| (s.slice_key, s.rev))
            .collect(),
        reason: "Keep my files; I will inspect these worktrees separately".into(),
    }
}

pub(super) async fn close_keeps_work_and_releases_only_execution() {
    for mode in ["staged-only", "noop", "return-fail", "acquire-fail"] {
        close_failed_work(mode).await;
    }
    for boundary in ["planner", "team"] {
        close_replays_partial_metadata(boundary).await;
    }
    close_refuses_foreign_and_undrained_work().await;
}

async fn close_failed_work(mode: &str) {
    let mut f = worker_fixture(mode);
    f.conn()
        .execute("UPDATE run SET max_repairs = 0", [])
        .unwrap();
    let start = f.approve().await;
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    let row = f.lease();
    let nodes = f.store.node_runs(f.turn.run_id).unwrap();
    let before_calls = calls(&f);
    let pool_calls = std::fs::read_to_string(f.dir.path().join("pool-calls")).unwrap();
    let lease = row
        .worktree_path
        .as_deref()
        .map_or_else(|| f.dir.path().join("worker-lease-1"), PathBuf::from);
    std::fs::create_dir_all(lease.join("dist")).unwrap();
    std::fs::write(
        lease.join("dist/kept.txt"),
        "ignored/disposable is not permission to discard\n",
    )
    .unwrap();
    std::fs::write(f.repo.join("human.txt"), "dirty solo work\n").unwrap();
    let index = git(&lease, &["write-tree"]);
    let head = git(&lease, &["rev-parse", "HEAD"]);
    let mut wrong = request(&f);
    wrong.expect_slices.insert("S1".into(), row.rev + 1);
    assert!(close_chat_team_build(f.store.path(), &wrong)
        .unwrap_err()
        .to_string()
        .contains("slices changed"));
    assert!(f.store.chat_build_closure(f.turn.run_id).unwrap().is_none());
    let mut wrong = request(&f);
    wrong.expect_plan_revision += 1;
    assert!(close_chat_team_build(f.store.path(), &wrong)
        .unwrap_err()
        .to_string()
        .contains("plan changed"));
    assert!(f.store.chat_build_closure(f.turn.run_id).unwrap().is_none());
    let target = request(&f);
    let report = close_chat_team_build(f.store.path(), &target).unwrap();
    assert_eq!(report.execution.phase, ChatTeamPhase::Finished);
    assert!(report.execution.quiescent && report.closure.finished_at.is_some());
    assert!(close_chat_team_build(f.store.path(), &target).is_err());
    assert_eq!(
        f.store.run(f.turn.run_id).unwrap().status,
        RunStatus::Cancelled
    );
    assert_eq!(
        serde_json::to_value(f.lease()).unwrap(),
        serde_json::to_value(&row).unwrap()
    );
    assert_eq!(f.store.node_runs(f.turn.run_id).unwrap().len(), nodes.len());
    for node in nodes {
        assert_eq!(
            f.store.node_run(node.id).unwrap().session_id,
            node.session_id
        );
    }
    assert_eq!(calls(&f), before_calls);
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("pool-calls")).unwrap(),
        pool_calls
    );
    assert_eq!(git(&lease, &["write-tree"]), index);
    assert_eq!(git(&lease, &["rev-parse", "HEAD"]), head);
    assert!(lease.join("dist/kept.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.repo.join("human.txt")).unwrap(),
        "dirty solo work\n"
    );
    let slice = &f.plan().bundle.unwrap().slices[0];
    assert!(slice.claimed_by.is_none());
    assert_eq!(
        slice.status,
        if row.commit_sha.is_some() {
            PlanStatus::InReview
        } else {
            PlanStatus::Blocked
        }
    );
    assert_eq!(f.store.retained_chat_builds(f.chat.id).unwrap().len(), 1);
    continue_in_solo(&mut f, &target);
    kept_path_cannot_be_reassigned(&mut f, &lease);
    if row.worktree_path.is_none() {
        protect_an_inspected_address(&mut f, &lease);
    }
}

fn continue_in_solo(f: &mut Fixture, target: &ChatBuildClose) {
    let chat = f.store.chat(f.chat.id).unwrap();
    f.store
        .set_chat_mode(chat.id, ChatMode::Single, chat.rev)
        .unwrap();
    let solo = f
        .store
        .begin_chat_turn(
            chat.id,
            "solo-after-close",
            "Continue without discarding the retained work",
            &f.registry,
        )
        .unwrap();
    assert_ne!(solo.run_id, f.turn.run_id);
    assert!(f
        .store
        .planning_access(chat.id, PlanActor::Agent(f.turn.node_id))
        .is_err());
    assert!(close_chat_team_build(f.store.path(), target).is_err());
    assert_eq!(
        f.store.chat(chat.id).unwrap().active_node_id,
        Some(solo.node_id)
    );
    f.store
        .finish_chat_turn(chat.id, solo.node_id, NodeStatus::Done, None)
        .unwrap();
    f.store.archive_chat(chat.id, true).unwrap();
    assert_eq!(f.store.retained_chat_builds(chat.id).unwrap().len(), 1);
}

fn protect_an_inspected_address(f: &mut Fixture, lease: &Path) {
    let separate = f.dir.path().join("human-linked");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            separate.to_str().unwrap(),
            "HEAD",
        ],
    );
    let other = f
        .store
        .create_chat(NewChat {
            project_id: f.chat.project_id,
            workspace: separate,
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    assert!(f
        .store
        .begin_chat_turn(
            other.id,
            "linked",
            "wait until the acquisition is located",
            &f.registry
        )
        .is_err());
    let mut inspected = ChatKeptPath {
        chat_id: f.chat.id,
        run_id: f.turn.run_id,
        slice_key: "S1".into(),
        expect_slice_revision: f.lease().rev,
        worktree: f.repo.clone(),
        reason: "I inspected the pool and found the kept acquisition here".into(),
    };
    assert!(
        f.store.protect_kept_chat_path(&inspected).is_err(),
        "must not reserve the source checkout"
    );
    lease.clone_into(&mut inspected.worktree);
    let saved = f.store.protect_kept_chat_path(&inspected).unwrap();
    assert!(crate::same_worktree(
        saved.worktree_path.as_deref().unwrap(),
        &lease.to_string_lossy()
    ));
    assert_eq!(saved.lease_state, "retained");
    assert!(
        f.store.protect_kept_chat_path(&inspected).is_err(),
        "cannot replace an address or replay a stale inspection"
    );
    let solo = f
        .store
        .begin_chat_turn(
            other.id,
            "linked",
            "this unrelated checkout is now known free",
            &f.registry,
        )
        .unwrap();
    f.store
        .finish_chat_turn(other.id, solo.node_id, NodeStatus::Done, None)
        .unwrap();
    assert!(lease.join("dist/kept.txt").exists());
    assert_eq!(
        f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().phase,
        ChatTeamPhase::Finished
    );
}

fn kept_path_cannot_be_reassigned(f: &mut Fixture, lease: &Path) {
    let alias = f.dir.path().join("lease-alias");
    std::os::unix::fs::symlink(lease, &alias).unwrap();
    let other = f
        .store
        .create_chat(NewChat {
            project_id: f.chat.project_id,
            workspace: alias.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    assert!(f
        .store
        .begin_chat_turn(other.id, "other", "write", &f.registry)
        .is_err());
    let run = f
        .store
        .create_run(f.chat.project_id, "legacy", RunTrigger::Manual)
        .unwrap();
    let node = f
        .store
        .dispatch(run.id, f.agent("backend").id, None, &f.registry)
        .unwrap();
    assert!(f
        .store
        .attach_worktree(node.id, &alias.to_string_lossy(), None, None)
        .is_err());
}

async fn close_replays_partial_metadata(boundary: &str) {
    let mut f = worker_fixture("exit-fail");
    f.add_slice("S2", vec!["crates/**"]);
    f.conn()
        .execute(
            "UPDATE run SET max_repairs = 0, on_failure = 'escalate'",
            [],
        )
        .unwrap();
    let start = f.approve().await;
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    let calls_before = calls(&f);
    let planner = rusqlite::Connection::open(f.store.planning_path().unwrap()).unwrap();
    if boundary == "planner" {
        planner.execute_batch("CREATE TRIGGER fail_close BEFORE UPDATE OF claimed_by ON slice WHEN OLD.key = 'S2' AND OLD.claimed_by IS NOT NULL AND NEW.claimed_by IS NULL BEGIN SELECT RAISE(ABORT, 'injected second claim release failure'); END;").unwrap();
    } else {
        f.conn().execute_batch("CREATE TRIGGER fail_close BEFORE UPDATE OF finished_at ON chat_build_closure BEGIN SELECT RAISE(ABORT, 'injected closure completion failure'); END;").unwrap();
    }
    assert!(close_chat_team_build(f.store.path(), &request(&f)).is_err());
    assert!(f
        .store
        .chat_build_closure(f.turn.run_id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    assert!(
        f.plan().bundle.unwrap().slices[0].claimed_by.is_none(),
        "the first engine commit survives the team rollback"
    );
    let close = request(&f);
    let resume = ChatBuildResume {
        target: close.target.clone(),
        slice_key: "S1".into(),
        expect_slice_revision: f.lease().rev,
    };
    assert!(resume_chat_team_slice(f.store.path(), &resume)
        .await
        .unwrap_err()
        .to_string()
        .contains("approval was withdrawn"));
    assert!(reconcile_chat_team_build(f.store.path(), &close.target)
        .await
        .unwrap_err()
        .to_string()
        .contains("approval was withdrawn"));
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
    if boundary == "planner" {
        planner.execute_batch("DROP TRIGGER fail_close").unwrap();
    } else {
        f.conn().execute_batch("DROP TRIGGER fail_close").unwrap();
        // An external owner arrives after irreversible close intent. A replay must
        // keep that claim untouched and report it, not trap the chat forever.
        planner.execute("UPDATE slice SET claimed_by = 'external', status = 'active', branch = 'external-work' WHERE key = 'S1'", []).unwrap();
    }
    let report = close_chat_team_build(f.store.path(), &request(&f)).unwrap();
    if boundary == "team" {
        assert!(!report.closure.issues.is_empty());
    }
    assert!(f.plan().bundle.unwrap().slices.iter().all(|s| {
        if boundary == "team" && s.key == "S1" {
            s.claimed_by.as_deref() == Some("external")
                && s.status == PlanStatus::Active
                && s.branch.as_deref() == Some("external-work")
        } else {
            s.claimed_by.is_none() && s.status == PlanStatus::Blocked
        }
    }));
    assert_eq!(calls(&f), calls_before);
    assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_none());
    assert!(f.store.retained_chat_builds(f.chat.id).unwrap()[0]
        .slices
        .iter()
        .all(|s| s.lease_state == "retained"));
}

async fn close_refuses_foreign_and_undrained_work() {
    let mut f = worker_fixture("noop");
    let start = f.approve().await;
    let owner = f.store.claim_chat_build(&start).unwrap();
    assert!(close_chat_team_build(f.store.path(), &request(&f)).is_err());
    drop(owner);
    assert!(close_chat_team_build(f.store.path(), &request(&f))
        .unwrap_err()
        .to_string()
        .contains("certified drained"));
    recover_chat_team_processes(f.store.path(), &request(&f).target)
        .await
        .unwrap();
    let planner = rusqlite::Connection::open(f.store.planning_path().unwrap()).unwrap();
    planner
        .execute("UPDATE slice SET claimed_by = 'foreign'", [])
        .unwrap();
    assert!(close_chat_team_build(f.store.path(), &request(&f))
        .unwrap_err()
        .to_string()
        .contains("foreign claim"));
    assert!(f.store.chat_build_closure(f.turn.run_id).unwrap().is_none());
    planner
        .execute("UPDATE slice SET claimed_by = NULL", [])
        .unwrap();
    let report = close_chat_team_build(f.store.path(), &request(&f)).unwrap();
    assert_eq!(
        report.slices[0].lease_state, "released",
        "provably unattempted pending work is not a retained acquisition"
    );
    assert!(f.store.retained_chat_builds(f.chat.id).unwrap().is_empty());
    assert!(calls(&f).is_empty());
    assert!(!f.dir.path().join("pool-calls").exists());
}
