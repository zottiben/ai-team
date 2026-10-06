//! Worktree proposals carry no dispatch authority and never rewrite old executions.
use ai_team_core::planning::PlanActor;
use ai_team_core::{
    ModelRegistry, NewChat, NewProject, NewRepo, NodeStatus, Provider, Reasoning, Store,
};

struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    a: i64,
    b: i64,
    target: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let target = root.path().join("target");
        std::fs::create_dir(&main).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(main.join("keep.txt"), "main edit").unwrap();
        std::fs::write(target.join("keep.txt"), "target edit").unwrap();
        let mut store = Store::init(&root.path().join("team.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "switch".into(),
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
        let a = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: main,
                provider: Provider::Local,
                model: "offline".into(),
                reasoning: Reasoning::High,
            })
            .unwrap()
            .id;
        let b = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: target.clone(),
                provider: Provider::Local,
                model: "offline".into(),
                reasoning: Reasoning::High,
            })
            .unwrap()
            .id;
        Self {
            _root: root,
            store,
            a,
            b,
            target,
        }
    }
}
#[test]
fn a_delayed_message_cannot_follow_a_checkout_switch() {
    let mut h = Fixture::new();
    let epoch = h.store.chat(h.a).unwrap().workspace_epoch;
    let request = h
        .store
        .request_chat_workspace(h.a, PlanActor::Human, &h.target)
        .unwrap();
    let updated = h
        .store
        .apply_chat_workspace(h.a, request.id, h.store.chat(h.a).unwrap().rev, &h.target)
        .unwrap();
    assert!(h
        .store
        .begin_chat_turn_at_epoch(
            h.a,
            "old checkout prompt",
            "delayed",
            &ModelRegistry::local_only(),
            epoch
        )
        .is_err());
    assert!(h.store.chat_turns(h.a).unwrap().is_empty());
    h.store
        .begin_chat_turn_at_epoch(
            h.a,
            "new checkout prompt",
            "new",
            &ModelRegistry::local_only(),
            updated.workspace_epoch,
        )
        .unwrap();
}

#[test]
fn even_a_round_trip_without_a_turn_invalidates_the_previous_session() {
    let mut h = Fixture::new();
    let registry = ModelRegistry::local_only();
    let first = h
        .store
        .begin_chat_turn(h.a, "first", "first", &registry)
        .unwrap();
    h.store
        .set_node_session(first.node_id, "old-session")
        .unwrap();
    h.store
        .finish_chat_turn(h.a, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let original = std::path::PathBuf::from(h.store.chat(h.a).unwrap().workspace_path);
    for target in [&h.target, &original] {
        let request = h
            .store
            .request_chat_workspace(h.a, PlanActor::Human, target)
            .unwrap();
        h.store
            .apply_chat_workspace(h.a, request.id, h.store.chat(h.a).unwrap().rev, target)
            .unwrap();
    }
    let next = h
        .store
        .begin_chat_turn(h.a, "next", "next", &registry)
        .unwrap();
    assert!(h.store.node_run(next.node_id).unwrap().session_id.is_none());
}

#[test]
fn proposal_waits_for_drain_preserves_history_and_starts_a_fresh_session() {
    let mut h = Fixture::new();
    let registry = ModelRegistry::local_only();
    let first = h
        .store
        .begin_chat_turn(h.a, "use target", "one", &registry)
        .unwrap();
    h.store
        .set_node_session(first.node_id, "old-session")
        .unwrap();
    let old = h.store.chat(h.a).unwrap().workspace_path;
    let request = h
        .store
        .request_chat_workspace(h.a, PlanActor::Agent(first.node_id), &h.target)
        .unwrap();
    let revision = h.store.chat(h.a).unwrap().rev;
    assert_eq!(h.store.chat(h.a).unwrap().workspace_path, old);
    assert!(h
        .store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .is_err());
    assert!(h
        .store
        .queue_chat_followup(
            h.a,
            first.node_id,
            "later",
            "queue",
            ai_team_core::FollowupKind::FollowUp
        )
        .is_err());
    h.store
        .finish_chat_turn(h.a, first.node_id, NodeStatus::Done, None)
        .unwrap();
    assert!(h
        .store
        .begin_chat_turn(h.a, "not yet", "two", &registry)
        .unwrap_err()
        .to_string()
        .contains("worktree request"));
    let revision = h.store.chat(h.a).unwrap().rev;
    let updated = h
        .store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .unwrap();
    assert_eq!(
        updated.workspace_path,
        h.target.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        h.store.run(first.run_id).unwrap().workspace_path.as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        h.store
            .node_run(first.node_id)
            .unwrap()
            .session_id
            .as_deref(),
        Some("old-session")
    );
    assert_eq!(
        h.store
            .node_run(first.node_id)
            .unwrap()
            .worktree_path
            .as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&old).join("keep.txt")).unwrap(),
        "main edit"
    );
    assert_eq!(
        std::fs::read_to_string(h.target.join("keep.txt")).unwrap(),
        "target edit"
    );
    let next = h
        .store
        .begin_chat_turn(h.a, "continue", "two", &registry)
        .unwrap();
    assert!(h.store.node_run(next.node_id).unwrap().session_id.is_none());
    assert_eq!(h.store.node_run(next.node_id).unwrap().stream_cursor, 0);
    assert_eq!(h.store.chat_turns(h.a).unwrap().len(), 2);
}
#[test]
fn destination_ownership_and_revision_are_checked_at_the_write() {
    let mut h = Fixture::new();
    let request = h
        .store
        .request_chat_workspace(h.a, PlanActor::Human, &h.target)
        .unwrap();
    let revision = h.store.chat(h.a).unwrap().rev;
    let mut other = Store::open(h.store.path()).unwrap();
    let turn = other
        .begin_chat_turn(h.b, "occupied", "other", &ModelRegistry::local_only())
        .unwrap();
    assert!(h
        .store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .unwrap_err()
        .to_string()
        .contains("owns this checkout"));
    other
        .finish_chat_turn(h.b, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    other.rename_chat(h.a, "renamed elsewhere").unwrap();
    assert!(h
        .store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .unwrap_err()
        .to_string()
        .contains("changed"));
    let revision = h.store.chat(h.a).unwrap().rev;
    h.store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .unwrap();
    assert!(h.store.chat_turns(h.a).unwrap().is_empty());
}
#[test]
fn requests_are_idempotent_scoped_and_cancellation_does_not_move_files() {
    let mut h = Fixture::new();
    let request = h
        .store
        .request_chat_workspace(h.a, PlanActor::Human, &h.target)
        .unwrap();
    assert_eq!(
        h.store
            .request_chat_workspace(h.a, PlanActor::Human, &h.target)
            .unwrap()
            .id,
        request.id
    );
    assert!(h.store.cancel_chat_workspace(h.b, request.id).is_err());
    let before = h.store.chat(h.a).unwrap().workspace_path;
    h.store.cancel_chat_workspace(h.a, request.id).unwrap();
    assert_eq!(h.store.chat(h.a).unwrap().workspace_path, before);
    assert_eq!(
        h.store
            .chat_workspace_request(h.a, request.id)
            .unwrap()
            .state,
        "cancelled"
    );
    let revision = h.store.chat(h.a).unwrap().rev;
    assert!(h
        .store
        .apply_chat_workspace(h.a, request.id, revision, &h.target)
        .is_err());
    h.store.archive_chat(h.a, true).unwrap();
    assert!(h
        .store
        .request_chat_workspace(h.a, PlanActor::Human, &h.target)
        .is_err());
}
#[test]
fn another_chats_agent_cannot_propose_or_approve_a_checkout() {
    let mut h = Fixture::new();
    let turn = h
        .store
        .begin_chat_turn(h.b, "occupied", "other", &ModelRegistry::local_only())
        .unwrap();
    assert!(h
        .store
        .request_chat_workspace(h.a, PlanActor::Agent(turn.node_id), &h.target)
        .is_err());
    assert!(h.store.chat_workspace_requests(h.a).unwrap().is_empty());
}
