use super::Harness;
use serde_json::json;

#[test]
fn toolbox_http_scans_without_writes_and_applies_only_the_saved_project_preview() {
    let repo = tempfile::tempdir().unwrap();
    let (h, _db) = Harness::with_repo(repo.path());
    let root = repo.path().canonicalize().unwrap();
    assert_eq!(h.get_anonymous("/api/projects/1/toolbox").status, 401);
    let scan = h.get("/api/projects/1/toolbox");
    assert_eq!(scan.status, 200, "{}", scan.body);
    assert!(scan.json()[0]["survey"].is_object());
    assert!(repo.path().read_dir().unwrap().next().is_none());
    let body = json!({"root":root,"selection":{"operation":"install","harnesses":["pi"],"hooks":[],"mcp":[],"skills":["pre-pr"],"scaffold":true}}).to_string();
    let proposed = h.post("/api/projects/1/toolbox/preview", &body);
    assert_eq!(proposed.status, 200, "{}", proposed.body);
    let id = proposed.json()["id"].as_i64().unwrap();
    assert!(!repo.path().join("AGENTS.md").exists());
    assert_eq!(
        h.post(
            &format!("/api/projects/2/toolbox/previews/{id}/apply"),
            "{}"
        )
        .status,
        400
    );
    std::fs::write(repo.path().join("AGENTS.md"), "An edit after preview").unwrap();
    let apply = h.post(
        &format!("/api/projects/1/toolbox/previews/{id}/apply"),
        "{}",
    );
    assert_eq!(apply.status, 200, "{}", apply.body);
    assert_eq!(apply.json()["state"], "refused");
    assert_eq!(apply.json()["outcome"]["applied"], json!([]));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("AGENTS.md")).unwrap(),
        "An edit after preview"
    );
    assert!(!repo.path().join(".agents").exists());
    assert_eq!(
        h.post(
            &format!("/api/projects/1/toolbox/previews/{id}/apply"),
            "{}"
        )
        .status,
        400
    );
    let fresh = h.post("/api/projects/1/toolbox/preview", &body).json();
    let id = fresh["id"].as_i64().unwrap();
    let apply = h.post(
        &format!("/api/projects/1/toolbox/previews/{id}/apply"),
        "{}",
    );
    assert_eq!(apply.json()["state"], "applied", "{}", apply.body);
    assert!(repo.path().join(".agents/skills/pre-pr/SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("AGENTS.md")).unwrap(),
        "An edit after preview"
    );
    assert_eq!(
        h.get(&format!("/api/projects/1/toolbox/previews/{id}"))
            .json()["state"],
        "applied"
    );
    assert_eq!(
        h.post("/api/projects/1/toolbox/preview", &body).json()["effects"],
        json!([])
    );
}
