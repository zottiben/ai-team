use super::Harness;
use serde_json::json;
#[test]
fn registry_routes_require_auth_keep_files_and_restore_the_hidden_registration() {
    let root = tempfile::tempdir().unwrap();
    let (h, _db) = Harness::with_repo(root.path());
    for url in [
        "/api/toolbox/user",
        "/api/toolbox/registry",
        "/api/toolbox/operations/registry",
        "/api/toolbox/operations/user/1",
    ] {
        assert_eq!(h.get_anonymous(url).status, 401);
    }
    std::fs::write(root.path().join("AGENTS.md"), "history stays").unwrap();
    let proposed = h.post(
        "/api/toolbox/registry/preview",
        r#"{"operation":"forget","projects":[1]}"#,
    );
    assert_eq!(proposed.status, 200, "{}", proposed.body);
    let id = proposed.json()["id"].as_i64().unwrap();
    assert_eq!(
        h.post(&format!("/api/toolbox/operations/user/{id}/apply"), "{}")
            .status,
        400
    );
    assert_eq!(
        h.post(
            &format!("/api/projects/1/toolbox/previews/{id}/apply"),
            "{}"
        )
        .status,
        400
    );
    assert_eq!(h.get("/api/projects").json().as_array().unwrap().len(), 1);
    let applied = h.post(
        &format!("/api/toolbox/operations/registry/{id}/apply"),
        "{}",
    );
    assert_eq!(applied.json()["state"], "applied", "{}", applied.body);
    assert!(h.get("/api/projects").json().as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(root.path().join("AGENTS.md")).unwrap(),
        "history stays"
    );
    assert_eq!(
        h.get("/api/toolbox/registry").json()["projects"][0]["status"],
        "archived"
    );
    let records = h.get("/api/toolbox/operations/registry").json();
    assert_eq!(records[0]["id"], id);
    assert!(records[0].get("changes").is_none());
    assert_eq!(
        h.post(
            &format!("/api/toolbox/operations/registry/{id}/apply"),
            "{}"
        )
        .status,
        400
    );
    let restored = h
        .post(
            "/api/toolbox/registry/preview",
            r#"{"operation":"restore","projects":[1]}"#,
        )
        .json();
    assert_eq!(
        h.post(
            &format!("/api/toolbox/operations/registry/{}/apply", restored["id"]),
            "{}"
        )
        .json()["state"],
        "applied"
    );
    assert_eq!(h.get("/api/projects").json().as_array().unwrap().len(), 1);
}
#[test]
fn discovery_does_not_register_or_save_roots_and_legacy_import_uses_a_project_receipt() {
    let repo = tempfile::tempdir().unwrap();
    let (h, _db) = Harness::with_repo(repo.path());
    let candidate = tempfile::tempdir().unwrap();
    std::fs::create_dir(candidate.path().join(".git")).unwrap();
    let found = h.post(
        "/api/toolbox/discover",
        &json!({"roots":[candidate.path()]}).to_string(),
    );
    assert_eq!(found.status, 200, "{}", found.body);
    assert_eq!(found.json()["repositories"].as_array().unwrap().len(), 1);
    assert!(found.json()["repositories"][0]["project"].is_null());
    assert_eq!(h.get("/api/projects").json().as_array().unwrap().len(), 1);
    assert_eq!(h.get("/api/toolbox/registry").json()["roots"], json!([]));
    std::fs::create_dir(repo.path().join(".pi")).unwrap();
    std::fs::write(
        repo.path().join(".pi/mcp.json"),
        r#"{"mcpServers":{"remote":{"url":"https://example.invalid/mcp","transport":"sse"}}}"#,
    )
    .unwrap();
    let proposed = h.post(
        "/api/projects/1/toolbox/preview",
        &json!({"root":repo.path().canonicalize().unwrap(),"selection":{"operation":"import_pi"}})
            .to_string(),
    );
    assert_eq!(proposed.status, 200, "{}", proposed.body);
    let id = proposed.json()["id"].as_i64().unwrap();
    assert!(!repo.path().join(".pi/mcp-adapter.json").exists());
    assert_eq!(
        h.post(
            &format!("/api/projects/1/toolbox/previews/{id}/apply"),
            "{}"
        )
        .json()["state"],
        "applied"
    );
    assert!(repo.path().join(".pi/mcp.json").is_file());
    assert!(
        std::fs::read_to_string(repo.path().join(".pi/mcp-adapter.json"))
            .unwrap()
            .contains("httpTransport")
    );
}
