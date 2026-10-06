use super::*;
use ai_team_core::{ModelRegistry, NewChat, NodeStatus, Provider, Reasoning, Store};

fn chat(store: &mut Store, project: i64, path: &std::path::Path) -> i64 {
    store
        .create_chat(NewChat {
            project_id: project,
            workspace: path.into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id
}
#[test]
fn today_is_authenticated_and_legacy_runs_are_not_open_chats() {
    let (h, dir) = Harness::with_store();
    assert_eq!(h.get_anonymous("/api/chat-today").status, 401);
    let answer = h.get("/api/chat-today");
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(
        answer.json(),
        serde_json::json!({"entries":[],"chats":0,"needs_attention":0,"working":0,"drafts":0})
    );
    assert!(
        Store::open(&dir.path().join("team.db"))
            .unwrap()
            .run(1)
            .is_ok(),
        "reading Today must not remove legacy history"
    );
}
#[test]
fn today_uses_each_chats_latest_attempt_and_excludes_archived_chats() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap().id;
    let first = chat(&mut store, project, dir.path());
    store.rename_chat(first, "Current conversation").unwrap();
    for (key, status) in [
        ("old failure", NodeStatus::Failed),
        ("fixed", NodeStatus::Done),
    ] {
        let turn = store
            .begin_chat_turn(first, key, key, &ModelRegistry::local_only())
            .unwrap();
        store
            .finish_chat_turn(first, turn.node_id, status, Some(key))
            .unwrap();
    }
    let second = chat(&mut store, project, dir.path());
    let turn = store
        .begin_chat_turn(
            second,
            "still needs help",
            "newer-failure",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    store
        .finish_chat_turn(
            second,
            turn.node_id,
            NodeStatus::Failed,
            Some("actual latest failure"),
        )
        .unwrap();
    let answer = h.get("/api/chat-today").json();
    assert_eq!(answer["chats"], 2);
    assert_eq!(answer["needs_attention"], 1);
    assert_eq!(answer["working"], 0);
    assert_eq!(answer["drafts"], 0);
    assert_eq!(answer["entries"][0]["chat_id"], second);
    assert_eq!(answer["entries"][0]["project_slug"], "widget");
    assert_eq!(answer["entries"][0]["state"], "failed");
    assert_eq!(answer["entries"][1]["chat_id"], first);
    assert_eq!(answer["entries"][1]["state"], "idle");
    assert!(!answer.to_string().contains("old failure"));
    store.archive_chat(second, true).unwrap();
    let answer = h.get("/api/chat-today").json();
    assert_eq!(answer["chats"], 1);
    assert_eq!(answer["needs_attention"], 0);
    assert_eq!(answer["entries"][0]["chat_id"], first);
    assert_eq!(
        store.chat_turns(second).unwrap().len(),
        1,
        "archiving filters the operating picture, not the history"
    );
}
#[test]
fn plan_questions_and_pending_checkout_changes_point_back_to_their_chat() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap().id;
    let id = chat(&mut store, project, dir.path());
    let made = h.post(
        &format!("/api/chats/{id}/plan"),
        r#"{"action":"create_plan","expect_revision":0,"title":"Owned plan"}"#,
    );
    assert_eq!(made.status, 200, "{}", made.body);
    let ask = serde_json::json!({"action":"open_question","expect_revision":made.json()["revision"],"body":"Choose the scope"});
    assert_eq!(
        h.post(&format!("/api/chats/{id}/plan"), &ask.to_string())
            .status,
        200
    );
    let answer = h.get("/api/chat-today").json();
    assert_eq!(answer["needs_attention"], 1);
    assert_eq!(answer["entries"][0]["questions"], 1);
    assert_eq!(answer["entries"][0]["panel"], "board");
    let other = tempfile::tempdir().unwrap();
    store
        .request_chat_workspace(id, ai_team_core::planning::PlanActor::Human, other.path())
        .unwrap();
    let answer = h.get("/api/chat-today").json();
    assert_eq!(answer["entries"][0]["state"], "checkout_change");
    assert_eq!(answer["entries"][0]["chat_id"], id);
}
