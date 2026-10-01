use super::*;

pub(super) async fn startup_does_not_spend_or_cancel_unclaimed_approval() {
    let mut f = worker_fixture("success");
    let start = f.approve().await;
    let approved = f.store.chat_team_run(f.turn.run_id).unwrap().unwrap();
    let plan = f.plan().revision;
    let slices = serde_json::to_value(f.store.chat_build_slices(f.turn.run_id).unwrap()).unwrap();
    let report = recover_abandoned_chat_teams(f.store.path()).await.unwrap();
    assert_eq!(report[0].state, ChatRecoveryState::PendingDispatch);
    assert_eq!(
        f.store.chat_team_run(f.turn.run_id).unwrap().unwrap().rev,
        approved.rev
    );
    let owner = f.store.claim_chat_build(&start).unwrap();
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        ChatRecoveryState::Active
    );
    drop(owner);
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        ChatRecoveryState::Recovered
    );
    assert_eq!(
        serde_json::to_value(f.store.chat_build_slices(f.turn.run_id).unwrap()).unwrap(),
        slices
    );
    assert_eq!(f.plan().revision, plan);
    assert!(calls(&f).is_empty());
    assert!(!f.dir.path().join("pool-calls").exists());
    assert_eq!(
        f.store.chat(f.chat.id).unwrap().active_node_id,
        Some(f.turn.node_id)
    );
    assert_eq!(
        f.store
            .chat_team_run(f.turn.run_id)
            .unwrap()
            .unwrap()
            .base_sha,
        approved.base_sha
    );
}
