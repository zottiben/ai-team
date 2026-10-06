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
    assert!(store.check_setup_pool(&[slot.clone()]).is_err());
    store.archive_chat(chat.id, true).unwrap();
    assert!(store.check_setup_pool(&[slot.clone()]).is_err());
    // An external lease protects the same slot from AWT's normal reuse.
    slot.lease_holder = Some("retained work".into());
    assert!(store.check_setup_pool(&[slot]).is_ok());
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
    assert!(store.check_setup_pool(&[slot]).is_err());
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
    assert!(store
        .request_workspace_setup(project, &dir.path().join("main"), "must-wait", "")
        .is_err());
    assert!(store.workspace_setups(project).unwrap().is_empty());
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
    assert!(store.check_setup_pool(&[slot]).is_err());
    assert!(store
        .request_workspace_setup(project, &dir.path().join("main"), "must-wait", "")
        .is_err());
}
