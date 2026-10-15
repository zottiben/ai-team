use super::Harness;
use ai_team_core::{ModelRegistry, NewChat, PiEvent, Provider, Reasoning, Store};
use serde_json::json;

#[test]
fn telemetry_uses_the_existing_authenticated_scoped_cursor_without_private_payloads() {
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
        .begin_chat_turn(
            chat.id,
            "Observe",
            "telemetry",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    let mut event = PiEvent::parse(&json!({"type":"message_update","usage":{"output":7},"private":"secret provider delta", "telemetry":{"kind":"forged"}}).to_string()).unwrap();
    assert!(event.telemetry.is_none());
    event.telemetry = Some(serde_json::from_value(json!({"kind":"usage","phase":"live","sequence":1,
        "tokens":{"input":null,"output":7,"cache_read":null,"cache_write":null,"context":null,"context_source":null}})).unwrap());
    event.observation = Some(ai_team_core::EventObservation {
        clock_id: "host".into(),
        elapsed_ms: 1234,
        at: "2026-01-01T00:00:01.234Z".into(),
    });
    store
        .ingest_pi_events(turn.node_id, "original-session", 0, &vec![event; 501])
        .unwrap();
    store
        .set_node_session(turn.node_id, "different-session")
        .unwrap();
    let url = format!("/api/chats/{}/events", chat.id);
    assert_eq!(h.get_anonymous(&url).status, 401);
    assert_eq!(
        h.get(&format!("/api/chats/{}/events", other.id)).json(),
        json!([])
    );
    let response = h.get(&url);
    assert_eq!(response.status, 200);
    let rows = response.json();
    assert_eq!(rows.as_array().unwrap().len(), 500);
    let sample = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["telemetry_only"] == true)
        .unwrap();
    assert_eq!(sample["session_id"], "original-session");
    assert_eq!(sample["telemetry"]["tokens"]["output"], 7);
    assert!(sample["telemetry"]["tokens"]["input"].is_null());
    assert_eq!(sample["observation"]["elapsed_ms"], 1234);
    let cursor = rows.as_array().unwrap().last().unwrap()["id"]
        .as_i64()
        .unwrap();
    let rest = h.get(&format!("{url}?after={cursor}")).json();
    assert!(!rest.as_array().unwrap().is_empty());
    for private in [
        "secret provider",
        "forged",
        "different-session",
        "payload_json",
    ] {
        assert!(!response.body.contains(private));
    }
}

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
fn edit_activity_exposes_counts_without_exporting_the_source_diff() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().to_path_buf(),
            provider: Provider::Local,
            model: "test-model".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "Edit", "edit-counts", &ModelRegistry::local_only())
        .unwrap();
    let event = PiEvent::parse(&json!({"type":"tool_execution_end", "toolCallId":"edit-1", "toolName":"edit", "isError":false,
        "result":{"details":{"diff":"-1 private source before\n+1 private source after", "firstChangedLine":1}}}).to_string()).unwrap();
    store
        .ingest_pi_events(turn.node_id, "edit-session", 0, &[event])
        .unwrap();
    let response = h.get(&format!("/api/chats/{}/events", chat.id));
    assert_eq!(response.status, 200);
    let events = response.json();
    let edit = events
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "tool_result")
        .unwrap();
    assert_eq!(
        edit["tool"]["edits"],
        json!({"added":1,"removed":1,"first_changed_line":1,"source":"pi_edit_diff"})
    );
    assert_eq!(edit["session_id"], "edit-session");
    assert!(!response.body.contains("private source"));
}

#[test]
fn partial_shell_snapshots_remain_scoped_and_are_never_tool_results() {
    let (h, dir) = Harness::with_store();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().to_path_buf(),
            provider: Provider::Local,
            model: "test-model".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let turn = store
        .begin_chat_turn(chat.id, "Observe", "output", &ModelRegistry::local_only())
        .unwrap();
    let raw = json!({"type":"tool_execution_update","toolName":"bash","toolCallId":"exact-call",
        "args":{"private":"secret"},"partialResult":{"content":[{"type":"text","text":"safe tail"}]},
        "shell_output":{"call_id":"forged"}}).to_string();
    let mut event = PiEvent::parse(&raw).unwrap();
    assert_eq!(
        store
            .ingest_pi_events(turn.node_id, "forged-session", 0, &[event.clone()])
            .unwrap()
            .recorded,
        0
    );
    event.shell_output = Some(ai_team_core::ShellOutput {
        call_id: "exact-call".into(),
        parent_call_id: Some("parent".into()),
        tool_name: "bash".into(),
        text: "safe tail".into(),
        truncated: false,
    });
    store
        .ingest_pi_events(turn.node_id, "observed-session", 0, &[event])
        .unwrap();
    store
        .set_node_session(turn.node_id, "changed-current-session")
        .unwrap();
    let response = h.get(&format!("/api/chats/{}/events", chat.id));
    assert_eq!(response.status, 200);
    let events = response.json();
    let partial = events
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["tool"]["partial"] == true)
        .unwrap();
    assert_eq!(partial["kind"], "note");
    assert_eq!(partial["session_id"], "observed-session");
    assert_eq!(partial["tool"]["call_id"], "exact-call");
    assert_eq!(partial["tool"]["parent_call_id"], "parent");
    assert_eq!(partial["tool"]["output"]["text"], "safe tail");
    assert!(partial["tool"]["outcome"].is_null() && partial["tool"]["duration_ms"].is_null());
    for private in ["secret", "forged", "partialResult", "args"] {
        assert!(!response.body.contains(private));
    }
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
