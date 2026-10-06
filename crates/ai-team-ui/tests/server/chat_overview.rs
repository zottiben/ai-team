//! The chat Overview: this chat's repository, seats and touched files - and never
//! another chat's.
//!
//! The regression these hold is the one the chat-first refactor exists to prevent: a
//! command surface that answers with the project's latest work rather than with the
//! conversation it is drawn under. Both routes are therefore driven with two chats in one
//! project, with the busier work in the chat that is *not* being asked about.

use super::*;
use ai_team_core::{EventKind, ModelRegistry, NewChat, NewEvent, Provider, Reasoning, Store};

fn checkout() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.path().join("README.md"), "# widget\n").unwrap();
    repo
}

fn chat(store: &mut Store, project: i64, workspace: &std::path::Path) -> i64 {
    store
        .create_chat(NewChat {
            project_id: project,
            workspace: workspace.to_path_buf(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id
}

/// One tool call, as Pi writes it: the tool and its arguments, raw.
fn tool_call(store: &mut Store, run: i64, node: i64, tool: &str, path: &str) {
    store
        .append_event(
            run,
            NewEvent::new(EventKind::ToolCall, tool)
                .on_node(node)
                .with(serde_json::json!({"toolName": tool, "args": {"path": path}})),
        )
        .unwrap();
    // The end of the same call carries the arguments again. Counting both would report
    // every file twice, so the Overview reads only the start.
    store
        .append_event(
            run,
            NewEvent::new(EventKind::ToolResult, format!("{tool} returned"))
                .on_node(node)
                .with(serde_json::json!({"toolName": tool, "args": {"path": path}})),
        )
        .unwrap();
}

/// Two chats in one project, both with work in flight, in separate checkouts because one
/// live turn owns a checkout. The busier work is deliberately in the chat that is *not*
/// being asked about.
struct Seeded {
    h: Harness,
    mine: i64,
    other: i64,
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
    _elsewhere: tempfile::TempDir,
}

fn seeded() -> Seeded {
    let repo = checkout();
    let (h, dir) = Harness::with_repo(repo.path());
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let mine = chat(&mut store, project.id, repo.path());
    // Its own checkout: one live turn owns a checkout, and this fixture needs both chats
    // working at once to prove neither can see the other.
    let elsewhere = checkout();
    let other = chat(&mut store, project.id, elsewhere.path());

    let turn = store
        .begin_chat_turn(mine, "Add a subtract", "mine", &ModelRegistry::local_only())
        .unwrap();
    tool_call(&mut store, turn.run_id, turn.node_id, "read", "src/lib.rs");
    tool_call(&mut store, turn.run_id, turn.node_id, "edit", "src/lib.rs");
    // Absolute, and spelled the way the seat's own checkout is: a lease path is stored
    // resolved, and `/var` and `/private/var` are different strings (D12).
    let lease = store.chat(mine).unwrap().workspace_path;
    tool_call(
        &mut store,
        turn.run_id,
        turn.node_id,
        "write",
        &format!("{lease}/README.md"),
    );
    // Outside the checkout entirely. A path the map cannot place is dropped, never
    // guessed onto the nearest node that looks like it.
    tool_call(&mut store, turn.run_id, turn.node_id, "read", "/etc/hosts");
    store
        .append_event(
            turn.run_id,
            NewEvent::new(EventKind::ToolCall, "bash")
                .on_node(turn.node_id)
                .with(serde_json::json!({"toolName":"bash","args":{"command":"cargo test"}})),
        )
        .unwrap();

    // The other chat is busier, and must not appear anywhere in this one's answer.
    let theirs = store
        .begin_chat_turn(
            other,
            "Rewrite the docs",
            "other",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    tool_call(
        &mut store,
        theirs.run_id,
        theirs.node_id,
        "write",
        "docs/theirs.md",
    );

    Seeded {
        h,
        mine,
        other,
        _dir: dir,
        _repo: repo,
        _elsewhere: elsewhere,
    }
}

#[test]
fn seat_speech_comes_from_assistant_payloads_not_human_or_host_summaries() {
    let Seeded {
        h, mine, _dir: dir, ..
    } = seeded();
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let node = store
        .node_run(store.chat(mine).unwrap().active_node_id.unwrap())
        .unwrap();
    assert!(h.get(&format!("/api/chats/{mine}/overview")).json()["seats"][0]["said"].is_null());
    store.append_event(node.run_id, NewEvent::new(EventKind::Note,"Misleading summary").on_node(node.id).by("assistant").with(serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"Actual assistant text"}]}}))).unwrap();
    store
        .append_event(
            node.run_id,
            NewEvent::new(EventKind::Note, "Host notice")
                .on_node(node.id)
                .by("ai-team"),
        )
        .unwrap();
    store.append_event(node.run_id, NewEvent::new(EventKind::Note,"User echo").on_node(node.id).with(serde_json::json!({"message":{"role":"user","content":[{"type":"text","text":"Not assistant text"}]}}))).unwrap();
    assert_eq!(
        h.get(&format!("/api/chats/{mine}/overview")).json()["seats"][0]["said"],
        "Actual assistant text"
    );
}

#[test]
fn a_seat_card_reports_the_command_it_is_running_now() {
    let Seeded { h, mine, .. } = seeded();
    let overview = h.get(&format!("/api/chats/{mine}/overview")).json();
    assert_eq!(overview["chat_id"], mine);
    assert_eq!(overview["mode"], "single");
    assert_eq!(overview["totals"]["turns"], 1);
    assert_eq!(
        overview["seats"].as_array().unwrap().len(),
        1,
        "a solo chat has one seat: {overview}"
    );
    let seat = &overview["seats"][0];
    assert_eq!(seat["role"], "assistant");
    assert_eq!(seat["status"], "running");
    assert_eq!(seat["live"], true);
    assert_eq!(
        seat["activity"]["summary"], "bash",
        "the newest durable action is the command it is running"
    );
    assert_eq!(seat["activity"]["detail"], "cargo test");
    assert_eq!(
        h.get_anonymous(&format!("/api/chats/{mine}/overview"))
            .status,
        401
    );
}

#[test]
fn touched_files_are_this_chats_evidence_and_nobody_elses() {
    let Seeded { h, mine, other, .. } = seeded();
    let overview = h.get(&format!("/api/chats/{mine}/overview")).json();
    let seat = &overview["seats"][0];

    let touched: Vec<(String, i64, i64)> = seat["touches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|touch| {
            (
                touch["path"].as_str().unwrap().to_string(),
                touch["reads"].as_i64().unwrap(),
                touch["writes"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(
        touched.contains(&("src/lib.rs".to_string(), 1, 1)),
        "one read and one edit of the same file is one path, counted both ways: {touched:?}"
    );
    assert!(
        touched.contains(&("README.md".to_string(), 0, 1)),
        "an absolute path inside the checkout is relative to it: {touched:?}"
    );
    assert!(
        !touched.iter().any(|(path, ..)| path.contains("hosts")),
        "a path outside the checkout is not placed on it: {touched:?}"
    );
    assert!(
        !touched.iter().any(|(path, ..)| path.contains("theirs")),
        "another chat's work is not this chat's: {touched:?}"
    );
    assert_eq!(
        seat["outside"], 1,
        "a path the checkout cannot place is counted, not silently dropped"
    );
    assert_eq!(overview["totals"]["files_touched"], 2);
    assert_eq!(overview["totals"]["files_written"], 2);

    // The chat that did none of this says so, rather than borrowing the busy one's work.
    let theirs = h.get(&format!("/api/chats/{other}/overview")).json();
    assert_eq!(theirs["totals"]["files_touched"], 1);
    assert_eq!(theirs["seats"][0]["touches"][0]["path"], "docs/theirs.md");
}

#[test]
fn the_map_is_coloured_by_the_seats_that_would_work_in_this_chat() {
    let repo = checkout();
    let (h, dir) = Harness::with_repo(repo.path());
    let mut store = Store::open(&dir.path().join("team.db")).unwrap();
    let project = store.find_project("widget").unwrap();
    let solo = chat(&mut store, project.id, repo.path());

    let view = h.get(&format!("/api/chats/{solo}/overview/map")).json();
    let zones = view["map"]["zones"].as_array().unwrap();
    assert_eq!(
        zones.len(),
        1,
        "a solo chat dispatches one seat, not the project roster: {view}"
    );
    assert_eq!(zones[0]["role"], "assistant");
    assert_eq!(zones[0]["zone"], "**");
    assert_eq!(
        view["map"]["unowned"], 0,
        "the solo seat works across the whole checkout"
    );
    let paths: Vec<&str> = view["map"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["path"].as_str())
        .collect();
    assert!(paths.contains(&"src/lib.rs"), "{paths:?}");

    // A team chat routes through the project's seats, so its zones are theirs.
    let team = chat(&mut store, project.id, repo.path());
    let revision = store.chat(team).unwrap().rev;
    store
        .set_chat_mode(team, ai_team_core::ChatMode::Team, revision)
        .unwrap();
    let view = h.get(&format!("/api/chats/{team}/overview/map")).json();
    let roles: Vec<&str> = view["map"]["zones"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|zone| zone["role"].as_str())
        .collect();
    assert!(
        roles.len() > 1 && !roles.contains(&"assistant"),
        "a team chat is routed by the roster's zones: {roles:?}"
    );

    assert_eq!(
        h.get_anonymous(&format!("/api/chats/{solo}/overview/map"))
            .status,
        401
    );
}
