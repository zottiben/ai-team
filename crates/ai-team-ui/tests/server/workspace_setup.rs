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
