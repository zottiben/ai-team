use super::*;

#[test]
fn setup_requires_approval_and_rejects_scope_injection_before_admission() {
    let (h, dir) = Harness::with_store();
    assert_eq!(
        h.get_anonymous("/api/chat-workspace-setups?project=widget")
            .status,
        401
    );
    let unapproved = h.post(
        "/api/chat-workspace-setups",
        r#"{"project":"widget","request_id":"fixture"}"#,
    );
    assert_eq!(unapproved.status, 400, "{}", unapproved.body);
    assert!(unapproved.body.contains("approve AWT"));
    let injected = h.post(
        "/api/chat-workspace-setups",
        r#"{"project":"widget","request_id":"fixture","approved":true,"repo_path":"/unrelated"}"#,
    );
    assert_eq!(injected.status, 422, "{}", injected.body);
    let store = ai_team_core::Store::open(&dir.path().join("team.db")).unwrap();
    assert!(store
        .workspace_setups(store.find_project("widget").unwrap().id)
        .unwrap()
        .is_empty());
}
#[test]
fn a_busy_setup_returns_a_terminal_receipt_and_does_not_block_new_chats() {
    let (h, dir) = Harness::with_store();
    let mut store = ai_team_core::Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap().id;
    store
        .attach_repo(
            project,
            ai_team_core::NewRepo {
                main_path: Some(dir.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let run = store
        .create_run(project, "active fixture", ai_team_core::RunTrigger::Manual)
        .unwrap();
    store
        .set_run_status(run.id, ai_team_core::RunStatus::Planning)
        .unwrap();
    store
        .set_run_supervisor(run.id, i64::from(std::process::id()))
        .unwrap();
    let body = r#"{"project":"widget","request_id":"refused-http","branch":"feature/pinned","approved":true}"#;
    let response = h.post("/api/chat-workspace-setups", body);
    assert_eq!(response.status, 200, "{}", response.body);
    let refused = response.json();
    assert_eq!(refused["state"], "failed");
    assert_eq!(refused["attempted"], false);
    assert!(refused["detail"]
        .as_str()
        .unwrap()
        .starts_with("Setup not started:"));
    assert!(refused["workspace_path"].is_null());
    assert_eq!(refused["steps"], serde_json::json!([]));
    store
        .set_run_status(run.id, ai_team_core::RunStatus::Done)
        .unwrap();
    let replay = h.post("/api/chat-workspace-setups", body);
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(
        replay.json(),
        refused,
        "the old request must not start after the blocker clears"
    );
    let inspect = h.post(
        &format!("/api/chat-workspace-setups/{}", refused["id"]),
        &serde_json::json!({"project":"widget", "revision":refused["rev"], "action":"inspect"})
            .to_string(),
    );
    assert_eq!(inspect.status, 200, "{}", inspect.body);
    assert_eq!(inspect.json(), refused);
    let revision = refused["rev"].as_i64().unwrap();
    for (action, revision) in [("inspect", revision + 1), ("retry_dependencies", revision)] {
        let denied = h.post(
            &format!("/api/chat-workspace-setups/{}", refused["id"]),
            &serde_json::json!({"project":"widget", "revision":revision, "action":action})
                .to_string(),
        );
        assert_eq!(denied.status, 400, "{}", denied.body);
    }
    assert_eq!(
        h.get("/api/chat-workspace-setups?project=widget").json()[0],
        refused
    );
    let chat = h.post(
        "/api/chats",
        r#"{"project":"widget","provider":"local","model":"fixture","reasoning":"high"}"#,
    );
    assert_eq!(chat.status, 200, "{}", chat.body);
    assert!(store
        .chat_turns(chat.json()["id"].as_i64().unwrap())
        .unwrap()
        .is_empty());
    assert_eq!(store.workspace_setups(project).unwrap().len(), 1);
    assert!(store.node_runs(run.id).unwrap().is_empty());
}

#[test]
fn setup_history_and_commands_are_project_and_revision_scoped() {
    let (h, dir) = Harness::with_store();
    let mut store = ai_team_core::Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap().id;
    let (receipt, _) = store
        .request_workspace_setup(project, dir.path(), "fixture", "")
        .unwrap();
    store
        .create_project(ai_team_core::NewProject {
            name: "other".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        h.get("/api/chat-workspace-setups?project=widget").json()[0]["id"],
        receipt.id
    );
    assert_eq!(
        h.get("/api/chat-workspace-setups?project=other").json(),
        serde_json::json!([])
    );
    for (project, revision) in [("other", receipt.rev), ("widget", receipt.rev + 1)] {
        let body = serde_json::json!({"project":project,"revision":revision,"action":"inspect"});
        let reply = h.post(
            &format!("/api/chat-workspace-setups/{}", receipt.id),
            &body.to_string(),
        );
        assert_eq!(reply.status, 400, "{}", reply.body);
    }
    assert_eq!(
        store.workspace_setup(project, receipt.id).unwrap().state,
        "pending"
    );
}
