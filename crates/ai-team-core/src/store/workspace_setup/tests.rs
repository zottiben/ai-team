use crate::{
    ModelRegistry, NewChat, NewProject, NewRepo, NodeStatus, PoolEntry, Provider, Reasoning,
    RoleModelDefault, RunStatus, RunTrigger, Store,
};

fn fixture() -> (tempfile::TempDir, Store, i64, PoolEntry) {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    let slot = dir.path().join("slot");
    std::fs::create_dir(&main).unwrap();
    std::fs::create_dir(&slot).unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .attach_repo(
            project.id,
            NewRepo {
                main_path: Some(main.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .seed_default_team(project.id, &RoleModelDefault::local_floor())
        .unwrap();
    let entry = PoolEntry {
        name: "1".into(),
        path: slot.canonicalize().unwrap().to_string_lossy().into(),
        status: "available".into(),
        lease_holder: None,
        processes: vec![],
        branch: None,
        main: false,
    };
    (dir, store, project.id, entry)
}

fn assert_protected(store: &Store, slot: &PoolEntry) {
    assert_eq!(
        store
            .setup_pool_exclusions(std::slice::from_ref(slot))
            .unwrap(),
        vec![slot.path.clone()]
    );
}

#[test]
fn an_unpinned_slot_keeps_awts_normal_reuse_policy() {
    let (_dir, store, _, slot) = fixture();
    assert!(store.setup_pool_exclusions(&[slot]).unwrap().is_empty());
}

#[test]
fn an_idle_chats_pinned_checkout_is_not_a_disposable_pool_slot() {
    let (_dir, mut store, project, mut slot) = fixture();
    let chat = store
        .create_chat(NewChat {
            project_id: project,
            workspace: (&slot.path).into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    assert_eq!(chat.active_node_id, None);
    assert_protected(&store, &slot);
    store.archive_chat(chat.id, true).unwrap();
    assert_protected(&store, &slot);
    // Keep the exclusion even if this external holder releases its lease.
    slot.lease_holder = Some("retained work".into());
    assert_protected(&store, &slot);
}

#[test]
fn planning_before_the_first_node_still_protects_a_pool_slot() {
    let (_dir, mut store, project, slot) = fixture();
    let run = store
        .create_run_in(
            project,
            "planning",
            RunTrigger::Manual,
            Some(slot.path.as_ref()),
        )
        .unwrap();
    store.set_run_status(run.id, RunStatus::Planning).unwrap();
    store
        .set_run_supervisor(run.id, i64::from(std::process::id()))
        .unwrap();
    assert!(store.node_runs(run.id).unwrap().is_empty());
    assert_protected(&store, &slot);
}

#[tokio::test]
async fn a_refused_setup_is_terminal_and_cannot_start_when_replayed_later() {
    let (dir, mut store, project, _slot) = fixture();
    let run = store
        .create_run(project, "planning", RunTrigger::Manual)
        .unwrap();
    store.set_run_status(run.id, RunStatus::Planning).unwrap();
    store
        .set_run_supervisor(run.id, i64::from(std::process::id()))
        .unwrap();
    let (refused, started) = store
        .request_workspace_setup(project, &dir.path().join("main"), "refused", "feature/one")
        .expect(
            "a known refusal must return a durable terminal receipt, not leave the UI uncertain",
        );
    assert!(!started);
    assert_eq!(refused.state, "failed");
    assert!(!refused.attempted);
    assert!(refused.workspace_path.is_none());
    assert!(refused.steps.is_empty());
    assert!(refused.detail.contains("execution"));
    store.set_run_status(run.id, RunStatus::Done).unwrap();
    let (replay, started) = store
        .request_workspace_setup(project, &dir.path().join("main"), "refused", "feature/one")
        .unwrap();
    assert!(!started);
    assert_eq!(replay.id, refused.id);
    assert_eq!(replay.state, "failed");
    assert!(store
        .request_workspace_setup(project, &dir.path().join("main"), "refused", "different")
        .is_err());
    assert!(store
        .claim_workspace_setup(project, refused.id, refused.rev, true)
        .is_err());
    let observed = crate::workspace_setup::run(store.path(), project, refused.id)
        .await
        .unwrap();
    assert_eq!(observed.rev, refused.rev);
    let inspected = crate::workspace_setup::inspect(store.path(), project, refused.id, refused.rev)
        .await
        .unwrap();
    assert_eq!(inspected.rev, refused.rev);
    assert!(store
        .workspace_setup_children(refused.id)
        .unwrap()
        .is_empty());
    let (fresh, started) = store
        .request_workspace_setup(
            project,
            &dir.path().join("main"),
            "fresh-explicit-approval",
            "feature/one",
        )
        .unwrap();
    assert!(started);
    assert_ne!(fresh.id, refused.id);
    assert_eq!(fresh.state, "pending");
}

#[test]
fn another_setup_refusal_does_not_replace_or_extend_the_existing_reservation() {
    let (dir, mut store, project, _slot) = fixture();
    let repo = dir.path().join("main");
    let (first, _) = store
        .request_workspace_setup(project, &repo, "first", "")
        .unwrap();
    let (refused, started) = store
        .request_workspace_setup(project, &repo, "second", "")
        .unwrap();
    assert!(!started);
    assert_eq!(refused.state, "failed");
    assert!(!refused.attempted);
    assert!(refused.detail.contains("existing AWT setup"));
    assert_eq!(
        store.workspace_setup(project, first.id).unwrap().state,
        "pending"
    );
    let owner = store
        .claim_workspace_setup(project, first.id, first.rev, false)
        .unwrap();
    store
        .finish_workspace_setup(&owner, "failed", "fixture ended before acquisition")
        .unwrap();
    let (replay, started) = store
        .request_workspace_setup(project, &repo, "second", "")
        .unwrap();
    assert!(!started);
    assert_eq!(replay.id, refused.id);
    assert!(!replay.attempted);
    assert_eq!(store.workspace_setups(project).unwrap().len(), 2);
    assert!(
        store
            .request_workspace_setup(project, &repo, "third", "")
            .unwrap()
            .1
    );
}

#[test]
fn a_planning_supervisor_blocks_setup_admission_before_any_node_exists() {
    let (dir, mut store, project, _slot) = fixture();
    let run = store
        .create_run(project, "planning", RunTrigger::Manual)
        .unwrap();
    store.set_run_status(run.id, RunStatus::Planning).unwrap();
    store
        .set_run_supervisor(run.id, i64::from(std::process::id()))
        .unwrap();
    let (refused, started) = store
        .request_workspace_setup(project, &dir.path().join("main"), "must-wait", "")
        .unwrap();
    assert!(!started);
    assert_eq!(refused.state, "failed");
    assert!(!refused.attempted);
    assert!(store
        .workspace_setup_children(refused.id)
        .unwrap()
        .is_empty());
    assert!(store.node_runs(run.id).unwrap().is_empty());
}

#[test]
fn a_blocked_node_keeps_its_pool_slot_and_project_reservation() {
    let (dir, mut store, project, slot) = fixture();
    let run = store
        .create_run(project, "retained", RunTrigger::Manual)
        .unwrap();
    let agent = store.agents(run.team_id.unwrap()).unwrap().remove(0);
    let node = store
        .dispatch(run.id, agent.id, None, &ModelRegistry::local_only())
        .unwrap();
    store
        .attach_worktree(node.id, &slot.path, None, None)
        .unwrap();
    store.set_node_status(node.id, NodeStatus::Blocked).unwrap();
    assert_protected(&store, &slot);
    let (refused, started) = store
        .request_workspace_setup(project, &dir.path().join("main"), "must-wait", "")
        .unwrap();
    assert!(!started);
    assert_eq!(refused.state, "failed");
    assert!(!refused.attempted);
    assert!(store
        .workspace_setup_children(refused.id)
        .unwrap()
        .is_empty());
    assert_eq!(store.node_run(node.id).unwrap().status, NodeStatus::Blocked);
}
