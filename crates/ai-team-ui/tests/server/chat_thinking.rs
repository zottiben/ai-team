use super::*;
use ai_team_core::{ModelRegistry, NewChat, NodeStatus, Provider, Reasoning, Store};

#[test]
fn chat_thinking_is_revision_checked_future_only_and_snapshotted_on_the_turn() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let path = format!("/api/chats/{}/thinking", chat.id);
    assert_eq!(h.get_anonymous(&path).status, 401);
    let request = serde_json::json!({"reasoning":"low","expect_revision":chat.rev});
    let changed = h.post(&path, &request.to_string());
    assert_eq!(changed.status, 200, "{}", changed.body);
    assert_eq!(changed.json()["reasoning"], "low");
    assert_eq!(h.post(&path, &request.to_string()).status, 400);
    assert!(store.chat_turns(chat.id).unwrap().is_empty());
    let turn = store
        .begin_chat_turn(chat.id, "work", "first", &ModelRegistry::local_only())
        .unwrap();
    let node = serde_json::to_value(store.node_run(turn.node_id).unwrap()).unwrap();
    assert_eq!(node["reasoning"], "low");
    let update =
        |revision| serde_json::json!({"reasoning":"medium","expect_revision":revision}).to_string();
    assert_eq!(
        h.post(&path, &update(store.chat(chat.id).unwrap().rev))
            .status,
        400
    );
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    assert_eq!(
        h.post(&path, &update(store.chat(chat.id).unwrap().rev))
            .status,
        200
    );
    let history = h.get(&format!("/api/chats/{}", chat.id)).json();
    assert_eq!(history["reasoning"], "medium");
    assert_eq!(history["turns"][0]["node"]["reasoning"], "low");
    let bad = serde_json::json!({"reasoning":"extreme","expect_revision":store.chat(chat.id).unwrap().rev});
    assert_eq!(h.post(&path, &bad.to_string()).status, 422);
    for (input, canonical) in [
        ("minimal", "minimal"),
        ("xhigh", "xhigh"),
        ("max", "max"),
        ("min", "minimal"),
        ("off", "none"),
    ] {
        let changed = h.post(&path, &serde_json::json!({"reasoning":input,"expect_revision":store.chat(chat.id).unwrap().rev}).to_string());
        assert_eq!(changed.status, 200, "{}", changed.body);
        assert_eq!(changed.json()["reasoning"], canonical);
        assert_eq!(
            store.node_run(turn.node_id).unwrap().reasoning,
            Some(Reasoning::Low)
        );
    }
    assert_eq!(
        h.get_anonymous("/api/models/thinking?provider=local&model=fixture")
            .status,
        401
    );
    assert_eq!(
        h.get("/api/models/thinking?provider=unknown&model=fixture")
            .status,
        400
    );
    assert_eq!(
        h.get("/api/models/thinking?provider=local&model=fixture&extra=1")
            .status,
        400
    );
    store.archive_chat(chat.id, true).unwrap();
    assert_eq!(
        h.post(&path, &update(store.chat(chat.id).unwrap().rev))
            .status,
        400
    );
}
