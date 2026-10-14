use super::Harness;
use ai_team_core::{ModelRegistry, NewChat, PiEvent, Provider, Reasoning, Store};
use serde_json::json;

fn offline_stream() -> Vec<PiEvent> {
    let mut events: Vec<_> = [
        json!({"type":"tool_execution_start", "toolCallId":"a", "toolName":"bash",
            "args":{"command":"printf hello", "private":"private argument"},
            "observation":{"clock_id":"forged", "elapsed_ms":10, "at":"2026-01-01T00:00:00.001Z"}}),
        json!({"type":"tool_execution_start", "toolCallId":"b", "toolName":"bash",
            "args":{"command":"printf world"}}),
        json!({"type":"tool_execution_end", "toolCallId":"b", "toolName":"bash",
            "isError":true, "durationMs":125.25,
            "result":{"content":[{"type":"text", "text":"failed safely"},
                {"type":"image", "data":"private image"},
                {"type":"thinking", "thinking":"private reasoning"}],
                "details":{"secret":"private details"}}}),
        json!({"type":"tool_execution_end", "toolCallId":"a", "toolName":"bash",
            "isError":false, "result":{"content":[{"type":"text", "text":"hello"}]}}),
    ]
    .into_iter()
    .map(|value| PiEvent::parse(&value.to_string()).unwrap())
    .collect();
    events[1].observation = Some(ai_team_core::EventObservation {
        clock_id: "host-fixture".into(),
        elapsed_ms: 42,
        at: "2026-01-01T00:00:00.042Z".into(),
    });
    events
}

#[test]
fn chat_activity_projects_exact_tool_identity_without_raw_provider_payloads() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let make = || NewChat {
        project_id: project.id,
        workspace: dir.path().to_path_buf(),
        provider: Provider::Local,
        model: "test-model".into(),
        reasoning: Reasoning::High,
    };
    let chat = store.create_chat(make()).unwrap();
    let other = store.create_chat(make()).unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "Inspect", "activity", &ModelRegistry::local_only())
        .unwrap();
    let events = offline_stream();
    store
        .set_node_session(turn.node_id, "new-current-session")
        .unwrap();
    store
        .ingest_pi_events(turn.node_id, "exact-session", 0, &events)
        .unwrap();
    let url = format!("/api/chats/{}/events", chat.id);
    assert_eq!(h.get_anonymous(&url).status, 401);
    assert_eq!(
        h.get(&format!("/api/chats/{}/events", other.id)).json(),
        json!([])
    );
    let response = h.get(&url);
    assert_eq!(response.status, 200);
    let visible = response.json();
    let tools: Vec<_> = visible
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"].as_str().unwrap().starts_with("tool_"))
        .collect();
    assert_eq!(tools.len(), 4);
    assert_eq!(tools[0]["tool"]["call_id"], "a");
    assert_eq!(tools[1]["tool"]["call_id"], "b");
    assert_eq!(tools[2]["tool"]["call_id"], "b");
    assert_eq!(tools[3]["tool"]["call_id"], "a");
    for (index, tool) in tools.iter().enumerate() {
        assert_eq!(tool["session_id"], "exact-session");
        assert_eq!(tool["node_run_id"], turn.node_id);
        if index == 1 {
            assert_eq!(
                tool["observation"],
                json!({"clock_id":"host-fixture", "elapsed_ms":42, "at":"2026-01-01T00:00:00.042Z"})
            );
        } else {
            assert!(
                tool["observation"].is_null(),
                "unobserved history cannot acquire precision"
            );
        }
        assert!(tool.get("payload").is_none());
    }
    assert_eq!(tools[0]["tool"]["target"]["text"], "printf hello");
    assert_eq!(tools[2]["tool"]["outcome"], "failed");
    assert_eq!(tools[2]["tool"]["duration_ms"], 125.25);
    assert_eq!(tools[2]["tool"]["output"]["text"], "failed safely");
    assert_eq!(tools[3]["tool"]["outcome"], "returned");
    assert!(tools[3]["tool"]["duration_ms"].is_null());
    for secret in [
        "private argument",
        "private image",
        "private reasoning",
        "private details",
        "forged",
    ] {
        assert!(
            !response.body.contains(secret),
            "{secret} leaked: {}",
            response.body
        );
    }
    let cursor = tools[1]["id"].as_i64().unwrap();
    let page = h.get(&format!("{url}?after={cursor}")).json();
    assert_eq!(page.as_array().unwrap().len(), 2);
    assert_eq!(page[0]["tool"]["call_id"], "b");
    store
        .ingest_pi_events(turn.node_id, "exact-session", 0, &events)
        .unwrap();
    assert_eq!(
        h.get(&url).json(),
        visible,
        "replay must not change retained evidence"
    );
}
