use super::Harness;
use ai_team_core::{NewChat, Provider, Reasoning, Store};

#[test]
fn pinned_checkout_evidence_is_authenticated_scoped_and_not_replaced_by_live_files() {
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "core.hooksPath", "/dev/null"],
        vec!["config", "core.fsmonitor", "false"],
    ] {
        let output = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(repo.path().join("source.txt"), "captured source\n").unwrap();
    let (app, db) = Harness::with_repo(repo.path());
    let mut store = Store::open(&db.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let make = || NewChat {
        project_id: project.id,
        workspace: repo.path().to_path_buf(),
        provider: Provider::Local,
        model: "offline-fixture".into(),
        reasoning: Reasoning::High,
    };
    let chat = store.create_chat(make()).unwrap().id;
    let other = store.create_chat(make()).unwrap().id;
    let state = app.get(&format!("/api/chats/{chat}/checkout")).json();
    let response = app.post(&format!("/api/chats/{chat}/checkout/preview"), &serde_json::json!({ "fingerprint": state["fingerprint"], "action": {"kind":"stage", "path":"source.txt"} }).to_string());
    assert_eq!(response.status, 200, "{}", response.body);
    let operation = response.json();
    assert_eq!(
        operation["evidence"]["files"][0]["file"]["hunks"][0]["lines"][0]["text"],
        "captured source"
    );
    let route = format!("/api/chats/{chat}/checkout/preview/{}", operation["id"]);
    assert_eq!(app.get_anonymous(&route).status, 401);
    let wrong_family = format!("/api/chats/{chat}/delivery/preview/{}", operation["id"]);
    assert_eq!(app.get_anonymous(&wrong_family).status, 401);
    assert_eq!(
        app.get(&wrong_family).status,
        400,
        "a checkout receipt is not draft-delivery evidence"
    );
    assert_eq!(
        app.get(&format!(
            "/api/chats/{other}/checkout/preview/{}",
            operation["id"]
        ))
        .status,
        400
    );
    std::fs::write(repo.path().join("source.txt"), "different current bytes\n").unwrap();
    assert_eq!(app.get(&route).json(), operation["evidence"]);
    let refused = app.post(
        &format!("/api/chats/{chat}/checkout/command"),
        &serde_json::json!({"action":"approve", "id":operation["id"], "revision":operation["rev"]})
            .to_string(),
    );
    assert_eq!(refused.status, 200, "{}", refused.body);
    assert_eq!(refused.json()["state"], "refused");
    let history = app.get(&format!("/api/chats/{chat}/checkout")).json();
    assert!(
        history["operations"][0].get("evidence").is_none(),
        "ordinary polls must not hydrate historical source"
    );
    assert_eq!(app.get(&route).json(), operation["evidence"]);
}
