use super::*;
use ai_team_core::{
    chat_images::{self, ImageInput, Prompt},
    ModelRegistry, NewChat, NodeStatus, Provider, Reasoning, Store,
};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC";

#[test]
fn images_are_authenticated_chat_scoped_and_absent_from_polling_payloads() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let make = || NewChat {
        project_id: project.id,
        workspace: dir.path().into(),
        provider: Provider::Local,
        model: "fixture".into(),
        reasoning: Reasoning::High,
    };
    let chat = store.create_chat(make()).unwrap();
    let other = store.create_chat(make()).unwrap();
    let input = ImageInput {
        name: "screenshot.png".into(),
        media_type: "image/png".into(),
        data: PNG.into(),
    };
    let turn = store
        .begin_chat_prompt(
            chat.id,
            "Inspect this",
            "image",
            &ModelRegistry::local_only(),
            Prompt {
                expected_reasoning: None,
                workspace_epoch: 0,
                images: chat_images::validate(vec![input]).unwrap(),
                publication: None,
            },
        )
        .unwrap();
    store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    let url = format!("/api/chats/{}/images/{}/0", chat.id, turn.node_id);
    assert_eq!(h.get_anonymous(&url).status, 401);
    let result = h.get(&url);
    assert_eq!(result.status, 200);
    assert_eq!(result.json()["data"], PNG);
    assert_eq!(
        h.get(&format!(
            "/api/chats/{}/images/{}/0",
            other.id, turn.node_id
        ))
        .status,
        400
    );
    assert_eq!(
        h.get(&format!("/api/chats/{}/images/{}/1", chat.id, turn.node_id))
            .status,
        400
    );
    let history = h.get(&format!("/api/chats/{}", chat.id));
    assert_eq!(
        history.json()["turns"][0]["images"][0]["name"],
        "screenshot.png"
    );
    assert!(!history.body.contains(PNG));
    let invalid = serde_json::json!({"message":"bad image","request_id":"invalid-image","workspace_epoch":0,"images":[{"name":"vector.svg","media_type":"image/svg+xml","data":PNG}]});
    assert_eq!(
        h.post(
            &format!("/api/chats/{}/messages", chat.id),
            &invalid.to_string()
        )
        .status,
        400
    );
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
}
