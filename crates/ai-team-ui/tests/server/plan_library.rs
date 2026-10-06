//! The plan board and the import approval, over a real socket.
//!
//! The source database here is a scratch fixture built with the engine in a temporary
//! directory. No route in these tests reads the operator's planner, and the assertions
//! include the one that matters most: the file the server read is byte-for-byte what it
//! was before the request.
use super::*;
use ai_team_core::{NewChat, Provider, Reasoning, Store};

/// A standalone planner database with one claimed, delivered slice in it.
fn source(path: &std::path::Path) -> i64 {
    use ai_planner_core as planner;
    let mut store = planner::Store::init(path).unwrap();
    store.set_actor("someone-else");
    let repo = store
        .ensure_repo(&planner::GitContext {
            repo_key: "git:github.com/acme/widget".into(),
            repo_name: "widget".into(),
            remote_url: None,
            main_path: std::path::PathBuf::from("/elsewhere/widget"),
            worktree: std::path::PathBuf::from("/elsewhere/widget"),
            branch: None,
            head_sha: None,
        })
        .unwrap();
    let plan = store
        .create_plan(planner::NewPlan {
            repo_id: repo.id,
            title: "Ship the widget".into(),
            slug: Some("ship-the-widget".into()),
            status: Some(planner::Status::Active),
            ..Default::default()
        })
        .unwrap();
    let slice = store
        .add_slice(planner::NewSlice {
            plan_id: plan.id,
            key: "PR1".into(),
            title: "Schema".into(),
            scope_md: Some("Schema work.\n\nTouches: src/**".into()),
            ..Default::default()
        })
        .unwrap();
    store
        .claim_slice(&slice, "/elsewhere/trees/pr1", Some("widget/pr1"))
        .unwrap();
    let id = plan.id;
    drop(store);
    id
}

fn chat(db: &std::path::Path, workspace: &std::path::Path) -> i64 {
    let mut store = Store::open(db).unwrap();
    let project = store.find_project("widget").unwrap();
    store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: workspace.to_path_buf(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id
}

#[test]
fn the_board_is_empty_until_a_plan_is_imported_into_a_chosen_chat() {
    let (h, dir) = Harness::with_store();
    let db = dir.path().join("team.db");
    let chat_id = chat(&db, dir.path());
    let planner_db = dir.path().join("standalone.db");
    let plan_id = source(&planner_db);
    let before = std::fs::read(&planner_db).unwrap();

    let board = h.get("/api/plan-library").json();
    assert_eq!(board["entries"], serde_json::json!([]));
    assert_eq!(board["destinations"][0]["chat_id"], chat_id);
    assert_eq!(board["destinations"][0]["project_slug"], "widget");

    // Reading the source is explicit, and says what is in it without choosing.
    let read = h.post(
        "/api/plan-library/source",
        &serde_json::json!({ "path": planner_db.to_string_lossy() }).to_string(),
    );
    assert_eq!(read.status, 200, "{}", read.body);
    let survey = read.json();
    assert_eq!(survey["plans"][0]["slug"], "ship-the-widget");
    assert_eq!(
        survey["plans"][0]["already_imported"],
        serde_json::Value::Null
    );

    let preview = h
        .post(
            "/api/plan-library/preview",
            &serde_json::json!({ "path": planner_db.to_string_lossy(), "plan_id": plan_id })
                .to_string(),
        )
        .json();
    assert_eq!(preview["counts"]["slices"], 1);
    assert_eq!(preview["evidence"][0]["key"], "PR1");
    assert_eq!(preview["fingerprint"].as_str().unwrap().len(), 64);

    let approve = |fingerprint: &str, chat: i64| {
        serde_json::json!({
            "path": planner_db.to_string_lossy(),
            "plan_id": plan_id,
            "chat_id": chat,
            "fingerprint": fingerprint,
        })
        .to_string()
    };
    let wrong = h.post(
        "/api/plan-library/import",
        &approve("not-the-preview", chat_id),
    );
    assert_eq!(wrong.status, 400, "{}", wrong.body);

    let imported = h.post(
        "/api/plan-library/import",
        &approve(preview["fingerprint"].as_str().unwrap(), chat_id),
    );
    assert_eq!(imported.status, 200, "{}", imported.body);
    assert_eq!(imported.json()["chat_id"], chat_id);
    assert_eq!(imported.json()["slug"], format!("chat-{chat_id}"));

    // It is that chat's plan, and the board opens it there.
    let plan = h.get(&format!("/api/chats/{chat_id}/plan")).json();
    assert_eq!(plan["bundle"]["plan"]["title"], "Ship the widget");
    let board = h.get("/api/plan-library").json();
    assert_eq!(board["entries"][0]["chat_id"], chat_id);
    assert_eq!(board["entries"][0]["project_slug"], "widget");
    assert!(board["entries"][0]["imported"]["source_path"]
        .as_str()
        .unwrap()
        .ends_with("standalone.db"));
    assert_eq!(board["destinations"], serde_json::json!([]));

    // A second approval of the same plan is refused, and the source is untouched.
    let again = h.post(
        "/api/plan-library/import",
        &approve(preview["fingerprint"].as_str().unwrap(), chat_id),
    );
    assert_eq!(again.status, 400, "{}", again.body);
    assert_eq!(std::fs::read(&planner_db).unwrap(), before);
    assert!(!dir.path().join("standalone.db-wal").exists());
}

#[test]
fn the_board_filters_by_project_and_status_and_needs_the_token() {
    let (h, dir) = Harness::with_store();
    let db = dir.path().join("team.db");
    let chat_id = chat(&db, dir.path());
    let planner_db = dir.path().join("standalone.db");
    let plan_id = source(&planner_db);

    assert_eq!(h.get_anonymous("/api/plan-library").status, 401);
    assert_eq!(
        h.get("/api/plan-library?status=nonsense").status,
        400,
        "an invented status is refused rather than silently ignored"
    );

    let preview = h
        .post(
            "/api/plan-library/preview",
            &serde_json::json!({ "path": planner_db.to_string_lossy(), "plan_id": plan_id })
                .to_string(),
        )
        .json();
    h.post(
        "/api/plan-library/import",
        &serde_json::json!({
            "path": planner_db.to_string_lossy(),
            "plan_id": plan_id,
            "chat_id": chat_id,
            "fingerprint": preview["fingerprint"],
        })
        .to_string(),
    );

    assert_eq!(
        h.get("/api/plan-library?project=widget").json()["entries"][0]["chat_id"],
        chat_id
    );
    assert_eq!(
        h.get("/api/plan-library?status=active").json()["entries"][0]["chat_id"],
        chat_id
    );
    assert_eq!(
        h.get("/api/plan-library?status=done").json()["entries"],
        serde_json::json!([])
    );
    assert_eq!(
        h.get("/api/plan-library?project=gadget").status,
        400,
        "a project that does not exist is an error, not an empty board"
    );
}

#[test]
fn archived_chat_plans_are_hidden_by_default_and_readable_on_request() {
    let (h, dir) = Harness::with_store();
    let db = dir.path().join("team.db");
    let active = chat(&db, dir.path());
    let archived = chat(&db, dir.path());
    for id in [active, archived] {
        let created = h.post(
            &format!("/api/chats/{id}/plan"),
            &serde_json::json!({
                "action": "create_plan", "expect_revision": 0, "title": format!("Plan {id}")
            })
            .to_string(),
        );
        assert_eq!(created.status, 200, "{}", created.body);
    }
    let mut store = Store::open(&db).unwrap();
    store.archive_chat(archived, true).unwrap();
    for url in [
        "/api/plan-library",
        "/api/plan-library?include_archived=false",
    ] {
        let board = h.get(url).json();
        assert_eq!(board["entries"].as_array().unwrap().len(), 1);
        assert_eq!(board["entries"][0]["chat_id"], active);
        assert_eq!(board["projects"][0]["plans"], 1);
    }
    let all = h.get("/api/plan-library?project=widget&status=draft&include_archived=true");
    assert_eq!(all.status, 200, "{}", all.body);
    let all = all.json();
    assert_eq!(all["entries"].as_array().unwrap().len(), 2);
    assert_eq!(all["projects"][0]["plans"], 2);
    assert!(all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["chat_id"] == archived && entry["chat_archived"] == true));
    let plan = h.get(&format!("/api/chats/{archived}/plan")).json();
    assert_eq!(plan["archived"], true);
    assert_eq!(plan["bundle"]["plan"]["title"], format!("Plan {archived}"));
    assert_eq!(
        h.get_anonymous("/api/plan-library?include_archived=true")
            .status,
        401
    );
    assert_eq!(
        h.get("/api/plan-library?include_archived=maybe").status,
        400
    );

    store.archive_chat(active, true).unwrap();
    let empty = h.get("/api/plan-library").json();
    assert_eq!(empty["entries"], serde_json::json!([]));
    assert_eq!(empty["projects"], serde_json::json!([]));
    assert!(
        store.chat(archived).unwrap().archived,
        "showing a plan must not restore its chat"
    );
    store.archive_chat(archived, false).unwrap();
    let restored = h.get("/api/plan-library").json();
    assert_eq!(restored["entries"].as_array().unwrap().len(), 1);
    assert_eq!(restored["entries"][0]["chat_id"], archived);
}

#[test]
fn a_request_carrying_anything_but_the_approval_is_refused() {
    let (h, dir) = Harness::with_store();
    let planner_db = dir.path().join("standalone.db");
    source(&planner_db);

    // `deny_unknown_fields` is the guarantee: a body smuggling another destination or a
    // command alongside the path is rejected rather than quietly ignored.
    let sneaky = serde_json::json!({
        "path": planner_db.to_string_lossy(),
        "command": "rm -rf /",
    })
    .to_string();
    assert_eq!(h.post("/api/plan-library/source", &sneaky).status, 422);

    let missing = serde_json::json!({ "path": dir.path().join("nope.db").to_string_lossy() });
    let response = h.post("/api/plan-library/source", &missing.to_string());
    assert_eq!(response.status, 400);
    assert!(response.json()["error"]
        .as_str()
        .unwrap()
        .contains("cannot read"));
}
