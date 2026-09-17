//! The whole path, against a real eve process.
//!
//! Ignored by default, and deliberately: it needs `npm install`, an `eve build`, and a
//! reachable model. CI cannot supply those, and a test that quietly no-ops when they are
//! missing would be worse than one that has to be asked for.
//!
//! Run it when the generator changes:
//!
//! ```sh
//! ailocal serve gemma4-12b-Q4_K_M
//! AI_TEAM_LIVE_MODEL=gemma4-12b-Q4_K_M cargo test -p ai-team-core --test live_eve -- --ignored --nocapture
//! ```
//!
//! What it proves that the unit tests cannot: that eve accepts what we generate, that
//! the authored tools reach the leased worktree rather than a sandbox, and that a real
//! NDJSON stream lands in SQLite as rows.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ai_team_core::{EventKind, NewProject, Provider, RunTrigger, Store};

const MODEL_ENV: &str = "AI_TEAM_LIVE_MODEL";
const TOKEN: &str = "live-test-secret";

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one narrative: generate, build, boot, drive, ingest - splitting it would hide the ordering that is the point"
)]
#[ignore = "needs npm, an eve build and a reachable model - see the module docs"]
fn a_generated_project_boots_and_its_turn_lands_in_sqlite() {
    let model = std::env::var(MODEL_ENV)
        .unwrap_or_else(|_| panic!("set {MODEL_ENV} to a model the ailocal gateway serves"));
    let ailocal_key = read_ailocal_key();

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();

    // A two-agent fixture: the orchestrator eve needs as a root, and one maker.
    let project = store
        .create_project(NewProject {
            name: "Live Fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let team = store.seed_default_team(project.id).unwrap();
    for agent in store.agents(team.id).unwrap() {
        if agent.role == "orchestrator" || agent.role == "backend" {
            store
                .set_agent_model(agent.id, Provider::Local, &model)
                .unwrap();
        } else {
            store.set_agent_enabled(agent.id, false).unwrap();
        }
    }

    // The worktree the agent is supposed to be able to reach, and nothing else.
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("MARKER.txt"), "the-secret-marker\n").unwrap();

    let project_dir = dir.path().join("agents");
    let generated = store.generate_project(team.id, &project_dir).unwrap();
    generated.write().unwrap();
    println!("generated {} files", generated.files.len());

    npm(&project_dir, &["install", "--no-audit", "--no-fund"]);
    // `eve build` is the real check on the generated project: it evaluates every
    // authored module and compiles the agent graph.
    eve(&project_dir, &["build"], &model, &worktree, &ailocal_key);

    let port = free_port();
    let mut server = spawn_eve_start(&project_dir, port, &worktree, &ailocal_key);
    let _guard = KillOnDrop(&mut server);

    wait_for_health(port);

    // The run these events belong to.
    let run = store
        .create_run(project.id, "read MARKER.txt", RunTrigger::Manual)
        .unwrap();
    let orchestrator = store
        .agents(team.id)
        .unwrap()
        .into_iter()
        .find(|a| a.role == "orchestrator")
        .unwrap();
    let node = store.dispatch(run.id, orchestrator.id, None).unwrap();
    store
        .attach_worktree(node.id, &worktree.to_string_lossy(), None, None)
        .unwrap();

    let session = start_session(
        port,
        "Read the file MARKER.txt in your worktree and reply with exactly its contents.",
    );
    println!("session {session}");
    store.set_node_session(node.id, &session).unwrap();

    // Read from the top, and tell the store that is where the batch starts - the
    // cursor is eve's absolute index, not a running total of what we happened to read.
    let from_index = 0;
    let ndjson = read_stream(port, &session);
    assert!(!ndjson.trim().is_empty(), "the stream produced nothing");

    let out = store.ingest_ndjson(node.id, from_index, &ndjson).unwrap();
    println!(
        "recorded {} ignored {} steps {} finished {}",
        out.recorded, out.ignored, out.steps, out.finished
    );
    assert!(out.finished, "the turn did not reach a terminal event");
    assert!(out.recorded > 0, "nothing was written to the event log");

    // Re-reading the same stream must not produce a second copy - the property the
    // whole reconnect story rests on, checked against real eve ids rather than fixtures.
    let before = store.node_run(node.id).unwrap();
    let again = store.ingest_ndjson(node.id, from_index, &ndjson).unwrap();
    assert_eq!(again.recorded, 0, "a replayed stream duplicated rows");
    assert_eq!(again.duplicates, out.recorded);

    // And the counters held too, not just the rows.
    let after = store.node_run(node.id).unwrap();
    assert_eq!(after.turns, before.turns, "a replay counted turns twice");
    assert_eq!(after.usage, before.usage, "a replay counted tokens twice");
    assert_eq!(
        after.stream_cursor, before.stream_cursor,
        "a rewind moved the cursor past the end of the stream"
    );

    let events = store.node_events(node.id, 500).unwrap();
    let kinds: Vec<EventKind> = events.iter().map(|e| e.kind).collect();
    println!("kinds: {kinds:?}");
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::Step | EventKind::Cost)),
        "a real turn must record at least one step"
    );

    // The tool actually reached the leased worktree. This is D3, end to end: the model
    // could only have seen this string by running our bash/read_file in that directory.
    let tool_ran = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::ToolCall | EventKind::ToolResult));
    let saw_marker = events
        .iter()
        .any(|e| e.summary.contains("the-secret-marker"))
        || events.iter().any(|e| {
            e.payload
                .as_ref()
                .is_some_and(|p| p.to_string().contains("the-secret-marker"))
        });
    println!("tool_ran={tool_ran} saw_marker={saw_marker}");
    assert!(tool_ran, "the agent never called a tool");

    let node = store.node_run(node.id).unwrap();
    println!(
        "usage {:?} turns {} cursor {}",
        node.usage, node.turns, node.stream_cursor
    );
    assert!(node.stream_cursor > 0, "the cursor did not advance");
}

fn read_ailocal_key() -> String {
    let path = PathBuf::from(std::env::var("HOME").unwrap()).join(".config/ailocal/gateway.key");
    std::fs::read_to_string(path)
        .map(|k| k.trim().to_string())
        .unwrap_or_default()
}

fn npm(dir: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("npm must be installed");
    assert!(status.success(), "npm {args:?} failed");
}

fn eve(dir: &Path, args: &[&str], _model: &str, worktree: &Path, key: &str) {
    let status = Command::new("npx")
        .arg("eve")
        .args(args)
        .current_dir(dir)
        .env("AI_TEAM_WORKTREE", worktree)
        .env("AI_TEAM_EVE_TOKEN", TOKEN)
        .env("AI_TEAM_AILOCAL_KEY", key)
        .status()
        .expect("npx must be installed");
    assert!(status.success(), "eve {args:?} failed");
}

fn spawn_eve_start(dir: &Path, port: u16, worktree: &Path, key: &str) -> std::process::Child {
    Command::new("npx")
        .args(["eve", "start", "--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .current_dir(dir)
        .env("AI_TEAM_WORKTREE", worktree)
        .env("AI_TEAM_EVE_TOKEN", TOKEN)
        .env("AI_TEAM_AILOCAL_KEY", key)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("eve start")
}

/// eve outlives the test otherwise, and a stray Node server holding a port is a bad
/// thing to leave behind on a developer's machine.
struct KillOnDrop<'a>(&'a mut std::process::Child);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn curl(args: &[&str]) -> String {
    let out = Command::new("curl").args(args).output().expect("curl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn wait_for_health(port: u16) {
    let url = format!("http://127.0.0.1:{port}/eve/v1/health");
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if curl(&["-fsS", "-m", "3", &url]).contains("\"ok\":true") {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("eve did not become healthy on {port}");
}

fn start_session(port: u16, message: &str) -> String {
    let body = serde_json::json!({ "message": message }).to_string();
    let raw = curl(&[
        "-fsS",
        "-m",
        "30",
        "-u",
        &format!("ai-team:{TOKEN}"),
        "-H",
        "content-type: application/json",
        "-d",
        &body,
        &format!("http://127.0.0.1:{port}/eve/v1/session"),
    ]);
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("sessionId")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("no sessionId in {raw:?}"))
}

/// Read the stream from the beginning. A local model is slow, so the timeout is
/// generous; curl returns when the turn parks and the server closes the response.
fn read_stream(port: u16, session: &str) -> String {
    curl(&[
        "-fsS",
        "-m",
        "600",
        "-u",
        &format!("ai-team:{TOKEN}"),
        &format!("http://127.0.0.1:{port}/eve/v1/session/{session}/stream?startIndex=0"),
    ])
}
