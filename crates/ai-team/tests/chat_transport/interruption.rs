use super::fixture::Fixture;
use ai_team_core::{
    drive_chat_team_build, resume_chat_team_slice, ChatBuildRecovery, ChatBuildResume,
    ChatBuildStart, ChatTeamPhase, NodeStatus, Store,
};
use std::{path::Path, time::Duration};

struct AbortTask(tokio::task::AbortHandle);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn build(
    f: &Fixture,
    store: &mut Store,
    start: ChatBuildStart,
) -> anyhow::Result<()> {
    std::fs::write(f.root.join("pause-maker"), "stop after real tools")?;
    let db = f.db.clone();
    let (chat, run, node) = (start.chat_id, start.run_id, start.node_id);
    let mut worker = tokio::spawn(async move { drive_chat_team_build(&db, start).await });
    let _abort = AbortTask(worker.abort_handle());
    // Keep the task in this scope even if the fixture's wait fails.
    let waited = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if f.root.join("maker-paused").exists() {
                return Ok(());
            }
            if worker.is_finished() {
                return Err(anyhow::anyhow!("build ended before the real maker paused"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    store.request_chat_stop(chat, node)?;
    match tokio::time::timeout(Duration::from_secs(30), &mut worker).await {
        Ok(result) => {
            let error = result?.expect_err("an interrupted maker must not report success");
            assert_eq!(error.to_string(), "S1: the worker did not complete its turn (cancelled): Stopped by you; unfinished work is kept for recovery.");
        }
        Err(error) => {
            worker.abort();
            let result = worker.await;
            anyhow::bail!("stopped build did not drain: {error}; aborted result: {result:?}");
        }
    }
    waited??;
    let execution = store.chat_team_run(run)?.unwrap();
    assert!(execution.quiescent);
    assert_eq!(execution.phase, ChatTeamPhase::Blocked);
    let slice = store.chat_build_slices(run)?.remove(0);
    assert_eq!(slice.lease_state, "retained");
    assert!(slice.commit_sha.is_none());
    let old = store.node_run(slice.maker_node_id.unwrap())?;
    let old_evidence = serde_json::to_value(&old)?;
    assert!(old.session_id.is_some());
    assert_eq!(old.status, NodeStatus::Cancelled);
    assert!(store
        .planning_access(chat, ai_team_core::planning::PlanActor::Agent(old.id))
        .is_err());
    assert_eq!(
        std::fs::read_to_string(
            Path::new(slice.worktree_path.as_ref().unwrap()).join("crates/answer.txt")
        )?,
        "GOOD\n"
    );
    std::fs::remove_file(f.root.join("pause-maker"))?;
    resume_chat_team_slice(
        &f.db,
        &ChatBuildResume {
            target: ChatBuildRecovery {
                chat_id: chat,
                run_id: run,
                node_id: node,
                expect_revision: execution.rev,
            },
            slice_key: slice.slice_key.clone(),
            expect_slice_revision: slice.rev,
        },
    )
    .await?;
    let after = store.chat_build_slices(run)?.remove(0);
    let maker = store.node_run(after.maker_node_id.unwrap())?;
    assert_ne!(maker.id, old.id);
    assert_eq!(maker.attempt, old.attempt + 1);
    assert_eq!(maker.session_id, old.session_id);
    assert_eq!(after.worktree_path, slice.worktree_path);
    assert_eq!(after.branch, slice.branch);
    assert_eq!(serde_json::to_value(store.node_run(old.id)?)?, old_evidence);
    Ok(())
}
