//! Explicit retained-work continuation uses real Git and the offline Pi transport.
use super::*;

struct AbortTask(tokio::task::AbortHandle);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn request(f: &Fixture) -> ChatBuildResume {
    let execution = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
    ChatBuildResume {
        target: ChatBuildRecovery {
            chat_id: f.chat.id,
            run_id: f.turn.run_id,
            node_id: f.turn.node_id,
            expect_revision: execution.rev,
        },
        slice_key: "S1".into(),
        expect_slice_revision: f.lease().rev,
    }
}

async fn stopped(mode: &str) -> Fixture {
    let mut f = worker_fixture(mode);
    let start = f.approve().await;
    let db = f.store.path().to_owned();
    let task = tokio::spawn(async move { drive_chat_team_build(&db, start).await });
    let _cleanup = AbortTask(task.abort_handle());
    let marker = match mode {
        "slow-maker" => "worker-waiting",
        "slow-verifier" => "verifier-waiting",
        _ => "gate-waiting",
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
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
        tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        f.store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .quiescent
    );
    std::fs::write(f.dir.path().join("worker-mode"), "success").unwrap();
    f
}

async fn stop_during_validation_does_not_grant_model_authority() {
    let mut f = stopped("slow-maker").await;
    let count = calls(&f).len();
    let target = request(&f);
    let db = f.store.path().to_owned();
    std::fs::write(f.dir.path().join("worker-mode"), "slow-status").unwrap();
    let task = tokio::spawn(async move { resume_chat_team_slice(&db, &target).await });
    let _cleanup = AbortTask(task.abort_handle());
    let marker = f.dir.path().join("status-waiting.pid");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.store.run(f.turn.run_id).unwrap().status,
        RunStatus::Blocked
    );
    assert!(f
        .store
        .planning_access(
            f.chat.id,
            PlanActor::Agent(f.lease().maker_node_id.unwrap())
        )
        .is_err());
    assert!(resume_chat_team_slice(f.store.path(), &request(&f))
        .await
        .unwrap_err()
        .to_string()
        .contains("controller or draining worker"));
    f.store
        .request_chat_stop(f.chat.id, f.turn.node_id)
        .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Stopped by you"), "{error}");
    assert_process_dead(&marker).await;
    assert_eq!(calls(&f).len(), count);
    assert!(
        f.store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .quiescent
    );
    assert_eq!(f.lease().lease_state, "retained");
}

async fn continuation_cleanup_keeps_every_failure() {
    let f = stopped("slow-maker").await;
    std::fs::write(f.dir.path().join("worker-mode"), "exit-fail").unwrap();
    f.conn().execute_batch("CREATE TRIGGER fail_resumed_slice BEFORE UPDATE ON chat_build_slice WHEN NEW.build_status = 'failed' BEGIN SELECT RAISE(ABORT, 'injected resumed slice failure'); END;
        CREATE TRIGGER fail_resumed_board BEFORE INSERT ON event WHEN NEW.summary LIKE 'S1: blocked%' BEGIN SELECT RAISE(ABORT, 'injected resumed board failure'); END;
        CREATE TRIGGER fail_resumed_finish BEFORE UPDATE ON chat_team_run WHEN NEW.phase = 'blocked' BEGIN SELECT RAISE(ABORT, 'injected resumed finish failure'); END;").unwrap();
    let error = resume_chat_team_slice(f.store.path(), &request(&f))
        .await
        .unwrap_err()
        .to_string();
    for cause in [
        "Implemented S1",
        "injected resumed slice failure",
        "injected resumed board failure",
        "injected resumed finish failure",
    ] {
        assert!(error.contains(cause), "lost {cause}: {error}");
    }
}

async fn refused(f: &Fixture, expected: &str) {
    let count = calls(f).len();
    let row = f.lease();
    let nodes = f.store.node_runs(f.turn.run_id).unwrap().len();
    let error = resume_chat_team_slice(f.store.path(), &request(f))
        .await
        .unwrap_err();
    assert!(error.to_string().contains(expected), "{expected}: {error}");
    assert_eq!(calls(f).len(), count);
    assert_eq!(f.store.node_runs(f.turn.run_id).unwrap().len(), nodes);
    assert_eq!(
        f.lease().rev,
        row.rev,
        "refusing a continuation does not mutate the slice"
    );
    assert_eq!(f.lease().maker_node_id, row.maker_node_id);
}

pub(super) async fn retained_work_continues_only_with_its_original_authority() {
    // A retryable infrastructure failure must not discard its still-usable claim.
    let mut f = worker_fixture("exit-fail");
    let start = f.approve().await;
    assert!(drive_chat_team_build(f.store.path(), start).await.is_err());
    std::fs::write(f.dir.path().join("worker-mode"), "success").unwrap();
    resume_chat_team_slice(f.store.path(), &request(&f))
        .await
        .unwrap();

    continuation_cleanup_keeps_every_failure().await;
    stop_during_validation_does_not_grant_model_authority().await;
    for mode in ["slow-maker", "slow-gate", "slow-verifier"] {
        let f = stopped(mode).await;
        let before = f.lease();
        let old = f.store.node_run(before.maker_node_id.unwrap()).unwrap();
        let path = Path::new(before.worktree_path.as_deref().unwrap());
        std::fs::write(path.join("crates/kept.txt"), "retained human edit\n").unwrap();
        let target = request(&f);
        resume_chat_team_slice(f.store.path(), &target)
            .await
            .unwrap();
        assert!(resume_chat_team_slice(f.store.path(), &target)
            .await
            .is_err());
        let row = f.lease();
        assert_eq!(row.lease_state, "released");
        assert_eq!(row.worktree_path, before.worktree_path);
        assert_eq!(row.branch, before.branch);
        let new = f.store.node_run(row.maker_node_id.unwrap()).unwrap();
        assert_ne!(new.id, old.id);
        assert_eq!(new.attempt, old.attempt + 1);
        assert_eq!(new.session_id, old.session_id);
        assert!(new.stream_cursor > old.stream_cursor);
        assert_eq!(f.store.node_run(old.id).unwrap().session_id, old.session_id);
        assert_eq!(
            git(
                &f.repo,
                &[
                    "show",
                    &format!("{}:crates/kept.txt", row.commit_sha.unwrap())
                ]
            ),
            "retained human edit"
        );
        assert!(!f.repo.join("crates/S1.txt").exists());
        let pool_calls = std::fs::read_to_string(f.dir.path().join("pool-calls")).unwrap();
        assert_eq!(
            pool_calls
                .lines()
                .filter(|s| s.starts_with("[\"get\","))
                .count(),
            1
        );
        assert_eq!(
            pool_calls
                .lines()
                .filter(|s| s.starts_with("[\"return\","))
                .count(),
            1
        );
        assert_eq!(
            f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().phase,
            ChatTeamPhase::Finished
        );
    }

    refusals_preserve_evidence().await;
}

async fn refusals_preserve_evidence() {
    let f = stopped("slow-maker").await;
    let path = PathBuf::from(f.lease().worktree_path.unwrap());
    let stale = request(&f);
    let mut wrong = stale.clone();
    wrong.target.chat_id += 1;
    assert!(resume_chat_team_slice(f.store.path(), &wrong)
        .await
        .is_err());
    wrong = stale.clone();
    wrong.expect_slice_revision += 1;
    assert!(resume_chat_team_slice(f.store.path(), &wrong)
        .await
        .is_err());
    std::fs::write(f.repo.join("human.txt"), "leave this alone").unwrap();
    refused(&f, "human checkout changed").await;
    std::fs::remove_file(f.repo.join("human.txt")).unwrap(); // Explicit operator resolution.
    std::fs::write(path.join("crates/staged.txt"), "staged only").unwrap();
    git(&path, &["add", "crates/staged.txt"]);
    std::fs::remove_file(path.join("crates/staged.txt")).unwrap();
    let index = git(&path, &["write-tree"]);
    refused(&f, "different staged tree").await;
    assert_eq!(git(&path, &["write-tree"]), index);
    git(&path, &["read-tree", "HEAD"]); // Operator explicitly selects the base index.
    std::fs::write(path.join("outside.txt"), "outside scope").unwrap();
    refused(&f, "outside the approved slice").await;
    std::fs::remove_file(path.join("outside.txt")).unwrap();
    f.conn()
        .execute(
            "UPDATE run SET budget_tokens = 0 WHERE id = ?1",
            [f.turn.run_id],
        )
        .unwrap();
    refused(&f, "run's token budget").await;
    f.conn()
        .execute(
            "UPDATE run SET budget_tokens = NULL, max_repairs = 0 WHERE id = ?1",
            [f.turn.run_id],
        )
        .unwrap();
    refused(&f, "attempt allowance is spent").await;
    f.conn()
        .execute(
            "UPDATE run SET max_repairs = 2, max_turns_node = 0 WHERE id = ?1",
            [f.turn.run_id],
        )
        .unwrap();
    refused(&f, "its cap is 0").await;
    f.conn()
        .execute(
            "UPDATE run SET max_turns_node = NULL WHERE id = ?1",
            [f.turn.run_id],
        )
        .unwrap();
    let maker = f.lease().maker_node_id.unwrap();
    f.conn()
        .execute(
            "UPDATE node_run SET session_retired_at = 'retired' WHERE id = ?1",
            [maker],
        )
        .unwrap();
    refused(&f, "original maker session").await;
    f.conn()
        .execute(
            "UPDATE node_run SET session_retired_at = NULL WHERE id = ?1",
            [maker],
        )
        .unwrap();
    let planner = rusqlite::Connection::open(f.store.planning_path().unwrap()).unwrap();
    planner
        .execute("UPDATE slice SET claimed_by = 'another owner'", [])
        .unwrap();
    refused(&f, "exact slice claim").await;
    assert_eq!(
        std::fs::read_to_string(path.join("crates/S1.txt")).unwrap(),
        "GOOD\n"
    );
    assert_eq!(calls(&f).len(), 1);
    assert!(!std::fs::read_to_string(f.dir.path().join("pool-calls"))
        .unwrap()
        .contains("return"));
}
