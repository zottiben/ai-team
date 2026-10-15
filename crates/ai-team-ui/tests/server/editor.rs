//! The operator editor's save path, over a real socket and a real checkout.
//!
//! The interesting half of an editor is not what it writes, it is what it refuses to
//! write. These drive the HTTP routes rather than the service, because the thing that
//! protects the file is the whole path - scope resolution, the revision the window
//! echoes back, and the status the browser branches on.

use super::Harness;

/// A git checkout with one committed file, and nothing else.
fn checkout() -> (tempfile::TempDir, std::path::PathBuf) {
    let source = tempfile::tempdir().unwrap();
    let repo = source.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(repo.join("notes.md"), "first\nsecond\n").unwrap();
    git(&["add", "notes.md"]);
    git(&["commit", "-qm", "notes"]);
    (source, repo)
}

#[test]
fn a_save_refuses_a_file_that_changed_on_disk_since_it_was_opened() {
    // The regression this slice exists for. The editor read `notes.md`, something else
    // wrote it, and the save must not carry the operator's stale buffer over the top of
    // work nobody has seen. Both versions have to survive the refusal.
    let (_source, repo) = checkout();
    let (app, _db) = Harness::with_repo(&repo);

    let opened = app.get("/api/editor/open?project=widget&path=notes.md");
    assert_eq!(opened.status, 200, "{}", opened.body);
    let opened = opened.json();
    assert_eq!(opened["text"], "first\nsecond\n");
    let base = opened["revision"].clone();
    assert!(
        base["hash"].as_str().is_some_and(|hash| hash.len() == 64),
        "an open has to fingerprint what it read: {base}"
    );

    std::fs::write(repo.join("notes.md"), "first\nsomebody else\n").unwrap();

    let proposed = "first\nsecond\nmine\n";
    let preview = app.post(
        "/api/editor/preview",
        &serde_json::json!({"project": "widget", "path": "notes.md", "base": base, "text": proposed})
            .to_string(),
    );
    assert_eq!(preview.status, 200, "{}", preview.body);
    let preview = preview.json();
    assert_eq!(
        preview["stale"], true,
        "the preview has to say the file moved under it: {preview}"
    );

    let refused = app.post(
        "/api/editor/save",
        &serde_json::json!({"project": "widget", "path": "notes.md", "base": base,
            "proposed_hash": preview["proposed"]["hash"], "text": proposed})
        .to_string(),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    let refused = refused.json();
    assert_eq!(refused["reason"], "changed_on_disk");
    assert_eq!(
        refused["message"],
        "Changed on disk since you opened it - reload or compare"
    );
    assert_eq!(
        refused["disk_text"]["text"], "first\nsomebody else\n",
        "a refusal has to hand back the version it refused to overwrite: {refused}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "first\nsomebody else\n",
        "the file on disk is the other writer's, untouched"
    );
}

#[test]
fn a_confirmed_save_writes_the_previewed_bytes_and_rebases_the_window() {
    let (_source, repo) = checkout();
    let (app, _db) = Harness::with_repo(&repo);
    let base = app
        .get("/api/editor/open?project=widget&path=notes.md")
        .json()["revision"]
        .clone();

    let proposed = "first\nsecond\nthird\n";
    let preview = app
        .post(
            "/api/editor/preview",
            &serde_json::json!({"project": "widget", "path": "notes.md", "base": base, "text": proposed})
                .to_string(),
        )
        .json();
    assert_eq!(preview["stale"], false);
    assert_eq!(preview["change"]["added"], 1);
    assert_eq!(preview["change"]["removed"], 0);
    assert_eq!(preview["proposed"]["bytes"], proposed.len());

    let saved = app.post(
        "/api/editor/save",
        &serde_json::json!({"project": "widget", "path": "notes.md", "base": base,
            "proposed_hash": preview["proposed"]["hash"], "text": proposed})
        .to_string(),
    );
    assert_eq!(saved.status, 200, "{}", saved.body);
    let saved = saved.json();
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        proposed
    );
    assert_eq!(saved["replaced"]["hash"], base["hash"]);
    assert_ne!(saved["revision"]["hash"], base["hash"]);
    assert_eq!(
        saved["notified_chats"], 0,
        "no chat owns this checkout, so nothing is told: {saved}"
    );
}

#[test]
fn a_saved_file_leaves_its_previous_version_where_the_window_can_read_and_discard_it() {
    // The save keeps the file it displaced, because a descriptor another process is
    // holding keeps writing into it. Keeping it silently would be no better than
    // deleting it, so the response names it and there are routes to read and drop it.
    let (_source, repo) = checkout();
    let (app, _db) = Harness::with_repo(&repo);
    let base = app
        .get("/api/editor/open?project=widget&path=notes.md")
        .json()["revision"]
        .clone();

    let proposed = "first\nsecond\nthird\n";
    let preview = app
        .post(
            "/api/editor/preview",
            &serde_json::json!({"project": "widget", "path": "notes.md", "base": base, "text": proposed})
                .to_string(),
        )
        .json();
    let saved = app
        .post(
            "/api/editor/save",
            &serde_json::json!({"project": "widget", "path": "notes.md", "base": base,
                "proposed_hash": preview["proposed"]["hash"], "text": proposed})
            .to_string(),
        )
        .json();

    let kept = saved["preserved"].clone();
    assert!(!kept.is_null(), "the displaced version is named: {saved}");
    assert_eq!(kept["hash"], base["hash"]);
    assert_eq!(
        kept["in_checkout"], false,
        "and it is out of `git status`: {kept}"
    );
    assert_eq!(
        std::fs::read_to_string(kept["path"].as_str().unwrap()).unwrap(),
        "first\nsecond\n"
    );

    let listed = app.get("/api/editor/kept?project=widget");
    assert_eq!(listed.status, 200, "{}", listed.body);
    let listed = listed.json();
    assert_eq!(listed[0]["name"], kept["name"]);
    assert_eq!(listed[0]["origin"], "notes.md");

    let name = kept["name"].as_str().unwrap();
    let text = app.get(&format!("/api/editor/kept/text?project=widget&name={name}"));
    assert_eq!(text.status, 200, "{}", text.body);
    assert_eq!(text.json()["text"]["text"], "first\nsecond\n");

    let dropped = app.post(
        "/api/editor/kept/discard",
        &serde_json::json!({"project": "widget", "name": name}).to_string(),
    );
    assert_eq!(dropped.status, 200, "{}", dropped.body);
    assert_eq!(dropped.json()["discarded"], true);
    assert!(app.get("/api/editor/kept?project=widget").json()[0].is_null());

    // Untouched by any of it: the saved file itself.
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        proposed
    );
}

#[test]
fn a_kept_files_name_cannot_be_pointed_at_the_checkout() {
    let (_source, repo) = checkout();
    let (app, _db) = Harness::with_repo(&repo);

    let escape = app.post(
        "/api/editor/kept/discard",
        &serde_json::json!({"project": "widget", "name": "../../notes.md"}).to_string(),
    );
    assert_eq!(escape.status, 400, "{}", escape.body);
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn there_is_no_unchecked_write_route_left_to_reach() {
    // A safe save beside an unsafe one is an unsafe save. `POST /file` used to write
    // whatever it was given; it must now be routed nowhere rather than deprecated.
    let (_source, repo) = checkout();
    let (app, _db) = Harness::with_repo(&repo);
    let attempt = app.post(
        "/api/file",
        &serde_json::json!({"project": "widget", "path": "notes.md", "text": "clobbered\n"})
            .to_string(),
    );
    // Nothing handles it any more, so it falls through to the window's own HTML. What
    // matters is the disk: no route behind that path writes a byte.
    assert!(
        serde_json::from_str::<serde_json::Value>(&attempt.body).is_err(),
        "POST /api/file answered as an API: {}",
        attempt.body
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn a_path_that_leaves_the_checkout_is_refused_over_http_too() {
    let (source, repo) = checkout();
    let outside = source.path().join("outside.txt");
    std::fs::write(&outside, "not yours\n").unwrap();
    let (app, _db) = Harness::with_repo(&repo);

    let opened = app.get("/api/editor/open?project=widget&path=../outside.txt");
    assert_eq!(opened.status, 400, "{}", opened.body);
    let saved = app.post(
        "/api/editor/save",
        &serde_json::json!({"project": "widget", "path": "../outside.txt",
            "base": {"hash": "0".repeat(64), "bytes": 0, "device": 0, "inode": 0, "modified_ms": null},
            "proposed_hash": "0".repeat(64), "text": "mine\n"})
            .to_string(),
    );
    assert_eq!(saved.status, 400, "{}", saved.body);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "not yours\n");
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_checkout_is_shown_as_unopenable_rather_than_followed() {
    let (_source, repo) = checkout();
    std::os::unix::fs::symlink(repo.join("notes.md"), repo.join("alias.md")).unwrap();
    let (app, _db) = Harness::with_repo(&repo);

    let opened = app
        .get("/api/editor/open?project=widget&path=alias.md")
        .json();
    assert_eq!(opened["editable"], false);
    assert!(
        opened["reason"].as_str().unwrap().contains("symbolic link"),
        "{opened}"
    );
    assert!(opened["revision"].is_null());
}

#[test]
fn the_gutter_baseline_is_heads_text_and_an_untracked_file_says_so() {
    let (_source, repo) = checkout();
    std::fs::write(repo.join("new.md"), "brand new\n").unwrap();
    std::fs::write(repo.join("notes.md"), "first\nedited\n").unwrap();
    let (app, _db) = Harness::with_repo(&repo);

    let committed = app
        .get("/api/editor/baseline?project=widget&path=notes.md")
        .json();
    assert_eq!(committed["tracked"], true);
    assert_eq!(
        committed["text"], "first\nsecond\n",
        "the markers compare against HEAD, not against the file on disk"
    );

    let fresh = app
        .get("/api/editor/baseline?project=widget&path=new.md")
        .json();
    assert_eq!(fresh["tracked"], false);
    assert!(fresh["text"].is_null());
}

#[test]
fn a_chat_working_in_the_checkout_is_told_at_its_next_turn_and_not_before() {
    let (_source, repo) = checkout();
    let (app, db) = Harness::with_repo(&repo);
    let store_path = db.path().join("team.db");
    let chat = {
        let mut store = ai_team_core::Store::open(&store_path).unwrap();
        let project = store.find_project("widget").unwrap();
        store
            .create_chat(ai_team_core::NewChat {
                project_id: project.id,
                workspace: repo.clone(),
                provider: ai_team_core::Provider::Local,
                model: "fixture".into(),
                reasoning: ai_team_core::Reasoning::High,
            })
            .unwrap()
            .id
    };

    let base = app
        .get("/api/editor/open?project=widget&path=notes.md")
        .json()["revision"]
        .clone();
    let preview = app
        .post(
            "/api/editor/preview",
            &serde_json::json!({"project": "widget", "path": "notes.md", "base": base, "text": "edited\n"})
                .to_string(),
        )
        .json();

    {
        let store = ai_team_core::Store::open(&store_path).unwrap();
        assert_eq!(
            store.pending_editor_saves(chat).unwrap(),
            0,
            "a preview is not a save, and nothing is announced before one"
        );
    }

    let saved = app.post(
        "/api/editor/save",
        &serde_json::json!({"project": "widget", "path": "notes.md", "base": base,
            "proposed_hash": preview["proposed"]["hash"], "text": "edited\n"})
        .to_string(),
    );
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(saved.json()["notified_chats"], 1);

    let mut store = ai_team_core::Store::open(&store_path).unwrap();
    assert_eq!(store.pending_editor_saves(chat).unwrap(), 1);
    let turn = store
        .begin_chat_turn(
            chat,
            "what changed?",
            "what changed?",
            &ai_team_core::ModelRegistry::local_only(),
        )
        .unwrap();
    let notice = store
        .take_editor_save_notice(chat, turn.node_id)
        .unwrap()
        .unwrap();
    assert!(notice.contains("notes.md"), "{notice}");
}

#[test]
fn a_refused_save_tells_no_chat_anything() {
    let (_source, repo) = checkout();
    let (app, db) = Harness::with_repo(&repo);
    let store_path = db.path().join("team.db");
    let chat = {
        let mut store = ai_team_core::Store::open(&store_path).unwrap();
        let project = store.find_project("widget").unwrap();
        store
            .create_chat(ai_team_core::NewChat {
                project_id: project.id,
                workspace: repo.clone(),
                provider: ai_team_core::Provider::Local,
                model: "fixture".into(),
                reasoning: ai_team_core::Reasoning::High,
            })
            .unwrap()
            .id
    };
    let base = app
        .get("/api/editor/open?project=widget&path=notes.md")
        .json()["revision"]
        .clone();
    std::fs::write(repo.join("notes.md"), "somebody else\n").unwrap();

    let refused = app.post(
        "/api/editor/save",
        &serde_json::json!({"project": "widget", "path": "notes.md", "base": base,
            "proposed_hash": "0".repeat(64), "text": "mine\n"})
        .to_string(),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    let store = ai_team_core::Store::open(&store_path).unwrap();
    assert_eq!(store.pending_editor_saves(chat).unwrap(), 0);
}

#[test]
fn opening_a_file_that_is_not_text_offers_no_base_to_save_against() {
    let (_source, repo) = checkout();
    std::fs::write(repo.join("logo.png"), [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
    let (app, _db) = Harness::with_repo(&repo);

    let opened = app
        .get("/api/editor/open?project=widget&path=logo.png")
        .json();
    assert_eq!(opened["editable"], false);
    assert_eq!(opened["reason"], "not UTF-8 text");
    assert!(opened["revision"].is_null());
    assert_eq!(opened["text"], "");
}
