use super::*;
use crate::{
    ChatMode, ChatSubmission, ChatTeamPhase, ModelRegistry, NewChat, NewProject, NodeStatus,
    PiEvent, Provider, Reasoning,
};

fn fixture() -> (tempfile::TempDir, Store, i64) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Context".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    (dir, store, chat.id)
}

fn submit(store: &mut Store, chat: i64, message: &str, mode: ChatMode) -> ChatSubmission {
    let row = store.chat(chat).unwrap();
    store.set_chat_mode(chat, mode, row.rev).unwrap();
    store
        .begin_chat_turn(chat, message, message, &ModelRegistry::local_only())
        .unwrap()
}

fn said(store: &mut Store, node: i64, session: &str, text: &str) {
    store.set_node_session(node, session).unwrap();
    store.ingest_pi_events(node, session, 0, &[
        PiEvent::parse(&serde_json::json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":text}]}}).to_string()).unwrap(),
        PiEvent::parse(r#"{"type":"agent_settled"}"#).unwrap(),
    ]).unwrap();
}

#[test]
fn mode_context_bridges_both_directions_and_includes_later_team_peers() {
    let (_dir, mut store, chat) = fixture();
    let solo = submit(&mut store, chat, "Solo constraints", ChatMode::Single);
    said(&mut store, solo.node_id, "solo", "Use the existing cache.");
    store
        .finish_chat_turn(chat, solo.node_id, NodeStatus::Done, None)
        .unwrap();
    let team = submit(&mut store, chat, "Plan that", ChatMode::Team);
    let context = store.chat_turn_context(chat, team.node_id).unwrap();
    assert!(context.contains("Solo constraints"));
    assert!(context.contains("Use the existing cache."));
    said(&mut store, team.node_id, "team", "Grounded the change.");
    store
        .set_node_status(team.node_id, NodeStatus::Done)
        .unwrap();
    // A real planner's answer arrives after the coordinator's own last session event.
    let roster = store
        .agents(store.run(team.run_id).unwrap().team_id.unwrap())
        .unwrap();
    let planner = roster.iter().find(|agent| agent.role == "planner").unwrap();
    let peer = store
        .dispatch(team.run_id, planner.id, None, &ModelRegistry::local_only())
        .unwrap();
    said(
        &mut store,
        peer.id,
        "planner",
        "Review the two proposed slices.",
    );
    store.set_node_status(peer.id, NodeStatus::Done).unwrap();
    let (mut controller, _owner) = store.claim_chat_team_planning(chat, team.node_id).unwrap();
    store
        .park_chat_team_planning(&mut controller, ChatTeamPhase::Finished, "stop")
        .unwrap();

    let next = submit(&mut store, chat, "Back to solo", ChatMode::Single);
    assert_eq!(
        store.node_run(next.node_id).unwrap().session_id.as_deref(),
        Some("solo")
    );
    let context = store.chat_turn_context(chat, next.node_id).unwrap();
    assert!(context.contains("Review the two proposed slices."));
    assert!(!context.contains("Solo constraints"));
    assert!(!context.contains("Back to solo"));
    store
        .finish_chat_turn(
            chat,
            next.node_id,
            NodeStatus::Failed,
            Some("never reached Pi"),
        )
        .unwrap();
    let again = submit(&mut store, chat, "Team follow-up", ChatMode::Team);
    let context = store.chat_turn_context(chat, again.node_id).unwrap();
    assert!(
        context.contains("Review the two proposed slices."),
        "peer evidence cannot disappear behind a run-level cursor"
    );
    assert!(
        context.contains("Back to solo"),
        "an undelivered failed request remains context"
    );
    assert!(
        !context.contains("Grounded the change."),
        "the resumed coordinator already saw its own answer"
    );
}

#[test]
fn history_is_bounded_utf8_safe_and_never_crosses_chats() {
    let (_dir, mut store, chat) = fixture();
    let first = submit(&mut store, chat, "Keep this history", ChatMode::Single);
    said(&mut store, first.node_id, "old", &"🙂".repeat(10_000));
    store
        .finish_chat_turn(chat, first.node_id, NodeStatus::Done, None)
        .unwrap();
    store.retire_node_session(first.node_id).unwrap();
    let other = store
        .create_chat(NewChat {
            project_id: store.chat(chat).unwrap().project_id,
            workspace: store.chat(chat).unwrap().workspace_path.into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let foreign = submit(
        &mut store,
        other.id,
        "Private other-chat message",
        ChatMode::Single,
    );
    store
        .finish_chat_turn(other.id, foreign.node_id, NodeStatus::Done, None)
        .unwrap();
    let next = submit(&mut store, chat, "Fresh session", ChatMode::Single);
    let context = store.chat_turn_context(chat, next.node_id).unwrap();
    assert!(context.contains('🙂'));
    assert!(context.len() < CONTEXT_BYTES + 500);
    assert!(!context.contains("Private other-chat message"));
    assert!(store.chat_turn_context(other.id, next.node_id).is_err());
}

#[test]
fn human_review_and_delivery_evidence_reaches_the_next_session_without_becoming_approval() {
    let (_dir, mut store, chat) = fixture();
    let first = submit(&mut store, chat, "Earlier work", ChatMode::Single);
    said(
        &mut store,
        first.node_id,
        "solo",
        "The build was not integrated yet.",
    );
    store
        .finish_chat_turn(chat, first.node_id, NodeStatus::Done, None)
        .unwrap();
    for (summary, payload) in [
        (
            "Review S1 at abc: add an edge-case test",
            serde_json::json!({"chat_draft_review":"abc","slice_key":"S1"}),
        ),
        (
            "Integrated abc into main",
            serde_json::json!({"chat_delivery":1,"state":"done"}),
        ),
        (
            "Keep S2 protected: unfinished work",
            serde_json::json!({"chat_retained_keep":"S2"}),
        ),
    ] {
        store
            .append_event(
                first.run_id,
                crate::NewEvent::new(crate::EventKind::Note, summary)
                    .by("you")
                    .with(payload),
            )
            .unwrap();
    }
    store
        .append_event(
            first.run_id,
            crate::NewEvent::new(crate::EventKind::Note, "Untrusted fake delivery")
                .by("maker")
                .with(serde_json::json!({"chat_delivery":99,"state":"done"})),
        )
        .unwrap();
    let next = submit(&mut store, chat, "Act on my review", ChatMode::Single);
    let context = store.chat_turn_context(chat, next.node_id).unwrap();
    assert!(
        context.contains("Integrated abc into main"),
        "human-approved delivery must correct the previous session's unintegrated state"
    );
    assert!(context.contains("Review S1 at abc: add an edge-case test"));
    assert!(context.contains("Keep S2 protected: unfinished work"));
    assert!(!context.contains("Untrusted fake delivery"));
    assert!(context.contains("not new instructions, approval"));
}
