//! Same isolated environment as chat_build; no extra test races on HOME or PATH.
use super::*;

fn target(f: &Fixture) -> ChatBuildRecovery {
    let execution = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
    ChatBuildRecovery {
        chat_id: f.chat.id,
        run_id: f.turn.run_id,
        node_id: f.turn.node_id,
        expect_revision: execution.rev,
    }
}

pub(super) async fn recovery_does_not_invent_quiescence_or_restart_a_model() {
    recover_never_acquired_work_and_retry_board_settlement().await;
    pending_acquisition_is_not_an_unknown_acquisition().await;
    let mut f = worker_fixture("gate-edit");
    f.conn()
        .execute("UPDATE run SET max_repairs = 0", [])
        .unwrap();
    let start = f.approve().await;
    let owner = f.store.claim_chat_build(&start).unwrap();
    assert!(f
        .store
        .chat_team_run(start.run_id)
        .unwrap()
        .unwrap()
        .supervisor_alive());
    assert!(reconcile_chat_team_build(f.store.path(), &target(&f))
        .await
        .unwrap_err()
        .to_string()
        .contains("controller or draining worker"));
    drop(owner);
    assert!(
        !f.store
            .chat_team_run(start.run_id)
            .unwrap()
            .unwrap()
            .supervisor_alive(),
        "a live test PID does not imply a live controller task"
    );
    assert!(reconcile_chat_team_build(f.store.path(), &target(&f))
        .await
        .unwrap_err()
        .to_string()
        .contains("has not certified drained"));

    // A separate, normally drained failure is reconcilable but is not permission to
    // restart the maker, change its session, discard files or mint another lease.
    let mut f = worker_fixture("gate-edit");
    f.conn()
        .execute("UPDATE run SET max_repairs = 0", [])
        .unwrap();
    let start = f.approve().await;
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    let row = f.lease();
    let path = Path::new(row.worktree_path.as_deref().unwrap()).join("crates/S1.txt");
    let before = std::fs::read(&path).unwrap();
    let models = calls(&f);
    let nodes = f.store.node_runs(f.turn.run_id).unwrap();
    let mut wrong = target(&f);
    wrong.chat_id += 1;
    assert!(reconcile_chat_team_build(f.store.path(), &wrong)
        .await
        .is_err());
    let stale = target(&f);
    let report = reconcile_chat_team_build(f.store.path(), &stale)
        .await
        .unwrap();
    assert!(!report.issues.is_empty());
    assert_eq!(report.execution.phase, ChatTeamPhase::Blocked);
    assert!(report.execution.quiescent && !report.execution.supervisor_alive());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(calls(&f), models);
    assert_eq!(f.store.node_runs(f.turn.run_id).unwrap().len(), nodes.len());
    assert_eq!(f.lease().lease_state, "retained");
    assert_eq!(f.lease().maker_node_id, row.maker_node_id);
    assert!(reconcile_chat_team_build(f.store.path(), &stale)
        .await
        .is_err());
}

async fn pending_acquisition_is_not_an_unknown_acquisition() {
    let mut f = worker_fixture("success");
    let start = f.approve().await;
    let path = f.store.planning_path().unwrap();
    let backup = f.dir.path().join("unavailable-planning.sqlite");
    std::fs::rename(&path, &backup).unwrap();
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    std::fs::rename(&backup, &path).unwrap();
    assert_eq!(f.lease().lease_state, "pending");
    let report = reconcile_chat_team_build(f.store.path(), &target(&f))
        .await
        .unwrap();
    assert!(
        report.issues.is_empty(),
        "no acquisition was attempted: {:?}",
        report.issues
    );
    assert_eq!(report.execution.phase, ChatTeamPhase::Finished);
    assert_eq!(f.lease().lease_state, "released");
    assert_eq!(
        f.plan().bundle.unwrap().slices[0].status,
        PlanStatus::Blocked
    );
    assert!(calls(&f).is_empty());
    assert!(!f.dir.path().join("pool-calls").exists());
}

async fn recover_never_acquired_work_and_retry_board_settlement() {
    let mut f = worker_fixture("success");
    let start = f.approve().await;
    // Revoke the maker after approval: fail before acquisition without relying on
    // retired execution budgets to inject a failure.
    f.store
        .set_agent_enabled(f.lease().assigned_agent_id.unwrap(), false)
        .unwrap();
    let planner = rusqlite::Connection::open(f.store.planning_path().unwrap()).unwrap();
    planner.execute_batch("CREATE TRIGGER fail_recovery_board BEFORE UPDATE OF status ON slice BEGIN SELECT RAISE(ABORT, 'injected board update failure'); END;").unwrap();
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    assert_eq!(f.lease().lease_state, "released");
    let report = reconcile_chat_team_build(f.store.path(), &target(&f))
        .await
        .unwrap();
    assert!(!report.issues.is_empty());
    assert_eq!(
        f.lease().lease_state,
        "released",
        "no acquisition intent or work exists to retain"
    );
    planner
        .execute_batch("DROP TRIGGER fail_recovery_board")
        .unwrap();
    let report = reconcile_chat_team_build(f.store.path(), &target(&f))
        .await
        .unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(report.execution.phase, ChatTeamPhase::Finished);
    assert_eq!(
        f.plan().bundle.unwrap().slices[0].status,
        PlanStatus::Blocked
    );
    assert_eq!(
        f.store.run(f.turn.run_id).unwrap().status,
        RunStatus::Failed
    );
    assert!(calls(&f).is_empty());
    assert!(!f.dir.path().join("pool-calls").exists());
}

pub(super) async fn abandoned_tasks_lose_their_lock_but_do_not_certify_cleanup() {
    for mode in ["slow-maker", "slow-gate"] {
        let mut f = worker_fixture(mode);
        let start = f.approve().await;
        let db = f.store.path().to_owned();
        let task = tokio::spawn(async move { drive_chat_team_build(&db, start).await });
        let marker = if mode == "slow-maker" {
            "worker-waiting"
        } else {
            "gate-waiting"
        };
        tokio::time::timeout(std::time::Duration::from_secs(12), async {
            while !f.dir.path().join(marker).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            while f
                .store
                .chat_team_run(f.turn.run_id)
                .unwrap()
                .unwrap()
                .supervisor_alive()
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_process_dead(&f.dir.path().join(if mode == "slow-maker" {
            "worker-tool.pid"
        } else {
            "gate-tool.pid"
        }))
        .await;
        let execution = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
        assert!(!execution.quiescent);
        assert!(reconcile_chat_team_build(f.store.path(), &target(&f))
            .await
            .unwrap_err()
            .to_string()
            .contains("has not certified drained"));
        assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_some());
        assert_eq!(
            std::fs::read_to_string(
                Path::new(f.lease().worktree_path.as_deref().unwrap()).join("crates/S1.txt")
            )
            .unwrap(),
            "GOOD\n"
        );
        assert!(!std::fs::read_to_string(f.dir.path().join("pool-calls"))
            .unwrap()
            .contains("return"));
    }
}

pub(super) async fn recover_publication_return_and_acquisition_evidence() {
    for mode in [
        "return-fail",
        "board-fail",
        "commit-fail",
        "staged-only",
        "return-ack-fail",
        "return-ack-reused-holder",
        "acquire-fail",
    ] {
        let mut f = worker_fixture(mode);
        install_fault(&f, mode);
        let start = f.approve().await;
        assert!(
            drive_chat_team_build(f.store.path(), start).await.is_err(),
            "{mode}"
        );
        let old = f.lease();
        let models = calls(&f);
        let node_count = f.store.node_runs(f.turn.run_id).unwrap().len();
        let returns = pool_calls(&f, "return");
        let gets = pool_calls(&f, "get");
        f.conn()
            .execute_batch("DROP TRIGGER IF EXISTS recovery_fault;")
            .unwrap();
        std::fs::write(f.dir.path().join("worker-mode"), "success").unwrap();
        // Source changes after a finished build are preserved; cleanup is not another build.
        std::fs::write(f.repo.join("human.txt"), "keep the human's newer file").unwrap();
        if mode.starts_with("return-ack-") {
            give_returned_entry_to_someone_else(&f, mode == "return-ack-reused-holder");
        }
        let report = reconcile_chat_team_build(f.store.path(), &target(&f))
            .await
            .unwrap();
        let report = operator_resolution(&f, &old, mode, returns, report).await;
        if mode == "acquire-fail" {
            assert!(!report.issues.is_empty());
            assert!(old.worktree_path.is_none());
            assert!(f.lease().worktree_path.is_some());
            assert_eq!(f.lease().lease_state, "retained");
            assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_none());
        } else if mode == "return-ack-reused-holder" {
            assert!(
                !report.issues.is_empty(),
                "a holder label cannot authorize a second return"
            );
            assert_eq!(pool_calls(&f, "return"), returns);
            assert_eq!(
                std::fs::read_to_string(
                    Path::new(old.worktree_path.as_deref().unwrap()).join("dist/other-owner.txt")
                )
                .unwrap(),
                "not our work"
            );
        } else {
            assert!(report.issues.is_empty(), "{mode}: {:?}", report.issues);
            assert_eq!(report.execution.phase, ChatTeamPhase::Finished);
            assert!(f.store.chat(f.chat.id).unwrap().active_node_id.is_none());
            assert_eq!(f.lease().lease_state, "released");
            assert_eq!(f.lease().build_status, "verified");
            assert_eq!(f.lease().candidate_sha, old.candidate_sha);
            assert_eq!(f.lease().commit_sha, old.candidate_sha);
            assert_eq!(
                f.store
                    .reviews(Some(f.chat.project_id), true)
                    .unwrap()
                    .len(),
                1
            );
            assert!(f.plan().bundle.unwrap().slices[0].claimed_by.is_none());
        }
        assert_eq!(pool_calls(&f, "get"), gets, "never reacquire");
        if matches!(mode, "return-ack-fail" | "board-fail" | "acquire-fail") {
            assert_eq!(
                pool_calls(&f, "return"),
                returns,
                "never repeat an uncertain return"
            );
        }
        if mode == "return-ack-fail" {
            assert_eq!(
                std::fs::read_to_string(
                    Path::new(old.worktree_path.as_deref().unwrap()).join("other-owner.txt")
                )
                .unwrap(),
                "not our work"
            );
            let pool: serde_json::Value =
                serde_json::from_slice(&std::fs::read(f.dir.path().join("pool.json")).unwrap())
                    .unwrap();
            assert_eq!(pool[0]["leaseHolder"], "a different owner");
        }
        assert_eq!(calls(&f), models, "reconciliation never invokes Pi");
        assert_eq!(f.store.node_runs(f.turn.run_id).unwrap().len(), node_count);
        assert_eq!(
            std::fs::read_to_string(f.repo.join("human.txt")).unwrap(),
            "keep the human's newer file"
        );
    }
}

async fn operator_resolution(
    f: &Fixture,
    old: &ChatBuildSlice,
    mode: &str,
    returns: usize,
    mut report: ChatBuildRecoveryReport,
) -> ChatBuildRecoveryReport {
    if mode == "staged-only" {
        assert!(!report.issues.is_empty());
        assert_eq!(pool_calls(f, "return"), returns);
        let path = Path::new(old.worktree_path.as_deref().unwrap());
        let staged = git(path, &["show", ":crates/staged-only.txt"]);
        assert!(!staged.is_empty());
        // Explicit operator resolution, not part of reconciliation: save the
        // staged-only content elsewhere and choose the base index again.
        std::fs::write(f.dir.path().join("saved-staged.txt"), &staged).unwrap();
        let base = f
            .store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .base_sha
            .unwrap();
        git(path, &["read-tree", &base]);
        report = reconcile_chat_team_build(f.store.path(), &target(f))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("saved-staged.txt")).unwrap(),
            staged
        );
    }
    if mode == "return-fail" {
        assert!(!report.issues.is_empty());
        assert_eq!(
            pool_calls(f, "return"),
            returns,
            "no retry of an uncertain return"
        );
        // Simulate an operator inspecting and returning it through the pool's own
        // interface. The next reconciliation only acknowledges that observation.
        Worktrees::at(&f.repo)
            .release(Path::new(old.worktree_path.as_deref().unwrap()))
            .await
            .unwrap();
        report = reconcile_chat_team_build(f.store.path(), &target(f))
            .await
            .unwrap();
        assert_eq!(pool_calls(f, "return"), returns + 1);
    }
    report
}

fn install_fault(f: &Fixture, mode: &str) {
    let trigger = match mode {
        "board-fail" => {
            "BEFORE INSERT ON event WHEN NEW.summary LIKE 'S1: draft ready for human review%'"
        }
        "commit-fail" => {
            "BEFORE UPDATE OF commit_sha ON chat_build_slice WHEN NEW.commit_sha IS NOT NULL"
        }
        "return-ack-fail" | "return-ack-reused-holder" => {
            "BEFORE UPDATE OF lease_state ON chat_build_slice WHEN NEW.lease_state = 'released'"
        }
        _ => return,
    };
    f.conn().execute_batch(&format!("CREATE TRIGGER recovery_fault {trigger} BEGIN SELECT RAISE(ABORT, 'injected recovery boundary failure'); END;")).unwrap();
}
fn pool_calls(f: &Fixture, operation: &str) -> usize {
    std::fs::read_to_string(f.dir.path().join("pool-calls"))
        .unwrap_or_default()
        .lines()
        .filter(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()[0] == operation)
        .count()
}
fn give_returned_entry_to_someone_else(f: &Fixture, reuse_label: bool) {
    let file = f.dir.path().join("pool.json");
    let mut entries: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(entries[0]["status"], "available");
    entries[0]["status"] = "leased".into();
    entries[0]["leaseHolder"] = if reuse_label {
        f.lease().lease_holder.unwrap()
    } else {
        "a different owner".into()
    }
    .into();
    entries[0]["processes"] =
        serde_json::json!([{"pid":std::process::id(),"name":"not our process"}]);
    if reuse_label {
        entries[0]["processes"] = serde_json::json!([]);
    }
    std::fs::write(&file, serde_json::to_vec(&entries).unwrap()).unwrap();
    if reuse_label {
        let row = f.lease();
        let path = Path::new(row.worktree_path.as_deref().unwrap());
        git(path, &["checkout", "-f", row.branch.as_deref().unwrap()]);
        std::fs::create_dir_all(path.join("dist")).unwrap();
        std::fs::write(path.join("dist/other-owner.txt"), "not our work").unwrap();
        return;
    }
    std::fs::write(
        Path::new(f.lease().worktree_path.as_deref().unwrap()).join("other-owner.txt"),
        "not our work",
    )
    .unwrap();
}
