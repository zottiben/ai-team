//! Command pads over the real HTTP path, with a fixture shell instead of the operator's.
//!
//! The suite never starts the operator's login shell: it would source their rc files and
//! write their history (AGENTS.md). `SHELL` is pointed at a script that behaves like one
//! for the only part that matters here - `-l -c <command>` - which also proves ai-team
//! runs a pad the way their own terminal would rather than with a hardcoded `/bin/sh`.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ai_team_ui::{ServeOptions, Server, TOKEN_HEADER};

struct Harness {
    addr: SocketAddr,
    token: String,
    /// The same file the server has open. A test that needs to stand where a crashed or
    /// earlier app session stood writes through this rather than pretending.
    db: PathBuf,
    _runtime: tokio::runtime::Runtime,
}

struct Response {
    status: u16,
    body: String,
}

impl Response {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|error| {
            panic!("expected JSON, got {}: {error}", self.body);
        })
    }
}

impl Harness {
    fn request(&self, method: &str, path: &str, body: Option<&str>) -> Response {
        let mut socket = TcpStream::connect(self.addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let body = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{TOKEN_HEADER}: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.token,
            body.len(),
        );
        socket.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        socket.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        Response {
            status,
            body: body.to_owned(),
        }
    }

    fn get(&self, path: &str) -> Response {
        self.request("GET", path, None)
    }

    fn post(&self, path: &str, body: &str) -> Response {
        self.request("POST", path, Some(body))
    }

    /// A second connection to the app's own database, for the cases a request cannot
    /// reach: a press recorded by an app session that is no longer running.
    fn store(&self) -> ai_team_core::Store {
        ai_team_core::Store::open(&self.db).unwrap()
    }

    fn project(&self) -> i64 {
        self.store().find_project("widget").unwrap().id
    }

    fn sessions(&self) -> usize {
        self.get("/api/terminals?project=widget")
            .json()
            .as_array()
            .unwrap()
            .len()
    }

    fn deck(&self) -> serde_json::Value {
        self.get("/api/terminals/commands?project=widget").json()
    }

    fn row(&self, token: &str) -> serde_json::Value {
        self.deck()["history"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["token"] == token)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {token}"))
    }
}

/// The fixture login shell, installed once for the whole binary.
///
/// `SHELL` and `HOME` are process-wide, so each test setting its own would race the
/// others running beside it - and a test that lost that race would start the operator's
/// real shell. One script, set once, shared.
fn fixture_shell() {
    static SHELL: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        let home = tempfile::tempdir().unwrap();
        let shell = home.path().join("fixture-shell");
        std::fs::write(
            &shell,
            // Exactly the `-l -c <command>` contract `open_command` uses. The marker
            // proves the pad went through $SHELL rather than a hardcoded interpreter.
            "#!/bin/sh\nprintf 'fixture-shell %s\\n' \"$1\"\nexec /bin/sh -c \"$3\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("SHELL", &shell);
        std::env::set_var("HOME", home.path());
        home
    });
}

/// A server with a project whose checkout is `repo`.
fn harness(root: &Path, repo: &Path) -> Harness {
    fixture_shell();

    let path = root.join("team.db");
    let mut store = ai_team_core::Store::init(&path).unwrap();
    let project = store
        .create_project(ai_team_core::NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .attach_repo(
            project.id,
            ai_team_core::NewRepo {
                main_path: Some(repo.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())
        .unwrap();
    drop(store);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(Server::bind(ServeOptions {
            store: Some(ai_team_core::Store::open(&path).unwrap()),
            credentials: ai_team_core::CredentialStore::isolated(),
            ..Default::default()
        }))
        .unwrap();
    let addr = server.addr();
    let token = server.token().to_owned();
    runtime.spawn(async move {
        let _ = server.serve().await;
    });
    Harness {
        addr,
        token,
        db: path,
        _runtime: runtime,
    }
}

/// A checkout, canonical, because that is what the server resolves to and what a press
/// has to name to be admitted.
fn checkout(root: &Path) -> PathBuf {
    let repo = root.join("checkout");
    std::fs::create_dir(&repo).unwrap();
    repo
}

/// The press the window sends: what was confirmed, and the id it is known by.
fn press(id: &str, pad: &serde_json::Value, where_: &Path) -> String {
    serde_json::json!({
        "project": "widget",
        "request_id": id,
        "pad": pad["id"],
        "revision": pad["revision"],
        "confirmed": pad["command"],
        "checkout": where_.to_string_lossy(),
    })
    .to_string()
}

fn add_pad(harness: &Harness, label: &str, command: &str) -> serde_json::Value {
    let added = harness.post(
        "/api/terminals/pads",
        &serde_json::json!({ "project": "widget", "label": label, "command": command }).to_string(),
    );
    assert_eq!(added.status, 200, "{}", added.body);
    added.json()
}

/// Poll the deck until a command's row reports an outcome, or give up.
fn settled(harness: &Harness, token: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let row = harness.row(token);
        if row["state"] != "running" {
            return row;
        }
        assert!(Instant::now() < deadline, "{row}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_pad_runs_in_a_session_of_its_own_and_its_receipt_is_the_commands_own_exit_and_duration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    assert_eq!(
        harness.get("/api/terminals/pads?project=widget").json(),
        serde_json::json!([]),
        "a project has no pads until somebody adds one; a seeded command is one nobody chose"
    );
    let pad = add_pad(
        &harness,
        "Check",
        "printf 'one\\ntwo\\n'; sleep 0.05; exit 4",
    );
    assert_eq!(pad["revision"], 1);

    // The press carries the text that was confirmed, and the pty it starts is new.
    assert_eq!(harness.sessions(), 0);
    let started = harness.post(
        "/api/terminals/commands",
        &press("press-first-00000000", &pad, &repo),
    );
    assert_eq!(started.status, 200, "{}", started.body);
    let started = started.json();
    assert_eq!(started["state"], "running");
    assert_eq!(started["replayed"], false);
    assert_eq!(started["dispatch"], "started");
    assert!(started["finished_at"].is_null());

    let sessions = harness.get("/api/terminals?project=widget").json();
    let sessions = sessions.as_array().unwrap();
    assert_eq!(sessions.len(), 1, "a press opens a session of its own");
    assert_eq!(sessions[0]["kind"], "command");
    assert_eq!(sessions[0]["label"], "Check");
    assert_eq!(sessions[0]["id"], started["session_id"]);

    let row = settled(&harness, started["token"].as_str().unwrap());
    assert_eq!(row["state"], "finished");
    assert_eq!(row["exit_code"], 4, "the exit status is the command's own");
    assert_eq!(
        row["lines"], 3,
        "every line the session printed, the login shell's own banner included - a count \
         of what is on screen rather than a guess at which lines were the command's"
    );
    assert_eq!(row["lines_exact"], true);
    assert!(
        row["duration_ms"].as_i64().unwrap() >= 50,
        "a measured duration, not a guess: {row}"
    );

    // What the pad actually ran, read back through the ordinary output route.
    let output = harness
        .get(&format!(
            "/api/terminals/{}?cursor=0",
            started["session_id"].as_i64().unwrap()
        ))
        .json();
    assert!(
        output["text"]
            .as_str()
            .unwrap()
            .contains("fixture-shell -l"),
        "a pad runs through the operator's own shell, as a login shell: {output}"
    );
    assert!(output["text"].as_str().unwrap().contains("one"));
    assert_eq!(output["dropped"], 0);

    // Settling is a write that happens once, so a second poll does not re-record it.
    let again = settled(&harness, started["token"].as_str().unwrap());
    assert_eq!(again["finished_at"], row["finished_at"]);
    assert_eq!(again["exit_code"], row["exit_code"]);
}

#[test]
fn a_press_that_arrives_twice_runs_once_and_is_answered_with_the_first_receipt() {
    // The response to the first press is lost - a dropped connection, a reload, a double
    // click - and the window sends the same press again. A pad that ran twice here would
    // be a second `git push`, a second migration, a second four-minute build.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let tally = repo.join("ran");
    let pad = add_pad(
        &harness,
        "Check",
        &format!("printf 'x\\n' >> {}", tally.display()),
    );
    let body = press("press-repeated-00000", &pad, &repo);

    let first = harness.post("/api/terminals/commands", &body);
    assert_eq!(first.status, 200, "{}", first.body);
    let first = first.json();
    assert_eq!(first["replayed"], false);
    let token = first["token"].as_str().unwrap().to_owned();
    settled(&harness, &token);

    for attempt in 0..3 {
        let again = harness.post("/api/terminals/commands", &body);
        assert_eq!(again.status, 200, "{}", again.body);
        let again = again.json();
        assert_eq!(
            again["replayed"], true,
            "attempt {attempt} is the same press, not a new one"
        );
        assert_eq!(again["token"], first["token"]);
        assert_eq!(again["id"], first["id"]);
        assert_eq!(again["session_id"], first["session_id"]);
        assert_eq!(
            again["state"], "finished",
            "the receipt it already has, not a fresh `running`"
        );
    }

    assert_eq!(
        std::fs::read_to_string(&tally).unwrap(),
        "x\n",
        "the command ran once, however many times the press arrived"
    );
    assert_eq!(harness.sessions(), 1, "and opened one pty");
    assert_eq!(
        harness.deck()["history"].as_array().unwrap().len(),
        1,
        "one press, one row"
    );
}

#[test]
fn a_request_id_that_comes_back_carrying_different_bytes_is_refused_and_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let pad = add_pad(&harness, "Check", "true");
    let other = add_pad(
        &harness,
        "Other",
        &format!("printf 'x\\n' >> {}", repo.join("other-ran").display()),
    );
    let first = harness.post(
        "/api/terminals/commands",
        &press("press-collision-0000", &pad, &repo),
    );
    assert_eq!(first.status, 200, "{}", first.body);
    let token = first.json()["token"].as_str().unwrap().to_owned();

    // The same name, a different press. Answering with the first receipt would report
    // something that never ran; running it would be a press nobody made. Every field the
    // press is confirmed against is part of its identity, so differing in any of them is
    // differing.
    for differing in [
        press("press-collision-0000", &other, &repo),
        serde_json::json!({
            "project": "widget", "request_id": "press-collision-0000",
            "pad": pad["id"], "revision": pad["revision"],
            "confirmed": "rm -rf /", "checkout": repo.to_string_lossy(),
        })
        .to_string(),
        serde_json::json!({
            "project": "widget", "request_id": "press-collision-0000",
            "pad": pad["id"], "revision": 9,
            "confirmed": pad["command"], "checkout": repo.to_string_lossy(),
        })
        .to_string(),
    ] {
        let refused = harness.post("/api/terminals/commands", &differing);
        assert_eq!(refused.status, 400, "{}", refused.body);
        assert!(refused.body.contains("different press"), "{}", refused.body);
    }

    // A reused id naming another checkout never reaches that comparison: the checkout is
    // refused first, which is the same answer by a shorter road.
    let elsewhere = harness.post(
        "/api/terminals/commands",
        &press("press-collision-0000", &pad, &root),
    );
    assert_eq!(elsewhere.status, 400, "{}", elsewhere.body);
    assert!(
        elsewhere.body.contains("not the checkout you confirmed"),
        "{}",
        elsewhere.body
    );
    assert!(!repo.join("other-ran").exists());
    assert_eq!(harness.sessions(), 1);
    let history = harness.deck()["history"].as_array().unwrap().len();
    assert_eq!(history, 1, "a refusal before the record leaves no row");
    assert_eq!(harness.row(&token)["request_id"], "press-collision-0000");
}

#[test]
fn a_pad_that_changed_since_it_was_read_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let pad = add_pad(&harness, "Check", "true");
    let edited = harness.post(
        &format!("/api/terminals/pads/{}", pad["id"].as_i64().unwrap()),
        r#"{"project":"widget","label":"Check","command":"touch ran-the-wrong-command"}"#,
    );
    assert_eq!(edited.status, 200, "{}", edited.body);
    assert_eq!(edited.json()["revision"], 2, "an edit is counted");

    let refused = harness.post(
        "/api/terminals/commands",
        &press("press-changed-00000", &pad, &repo),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(refused.body.contains("nothing was run"), "{}", refused.body);

    // Edited back to the text that was confirmed is still an edit. The pad on screen is
    // not the pad that was read, and the revision is what says so.
    let reverted = harness.post(
        &format!("/api/terminals/pads/{}", pad["id"].as_i64().unwrap()),
        r#"{"project":"widget","label":"Check","command":"true"}"#,
    );
    assert_eq!(reverted.json()["revision"], 3);
    let stale = harness.post(
        "/api/terminals/commands",
        &press("press-reverted-00000", &pad, &repo),
    );
    assert_eq!(stale.status, 400, "{}", stale.body);
    assert!(
        stale.body.contains("edited since you read it"),
        "{}",
        stale.body
    );

    assert_eq!(harness.sessions(), 0, "a refused press opens no session");
    assert_eq!(
        harness.deck()["history"],
        serde_json::json!([]),
        "and leaves no receipt"
    );

    // A pad that is gone is the same refusal, not a silent no-op.
    harness.post(
        &format!("/api/terminals/pads/{}/remove", pad["id"].as_i64().unwrap()),
        r#"{"project":"widget"}"#,
    );
    let gone = harness.post(
        "/api/terminals/commands",
        &press("press-removed-00000", &pad, &repo),
    );
    assert_eq!(gone.status, 400, "{}", gone.body);
    assert!(!repo.join("ran-the-wrong-command").exists());
}

#[test]
fn a_press_naming_a_checkout_this_deck_has_left_runs_nothing() {
    // The confirmation names a checkout. If the deck moved while the dialog was open,
    // running "the current one" would run a command in a directory nobody confirmed.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let pad = add_pad(&harness, "Check", "true");
    let elsewhere = harness.post(
        "/api/terminals/commands",
        &press("press-elsewhere-0000", &pad, &root),
    );
    assert_eq!(elsewhere.status, 400, "{}", elsewhere.body);
    assert!(
        elsewhere.body.contains("not the checkout you confirmed"),
        "{}",
        elsewhere.body
    );
    assert_eq!(harness.sessions(), 0);
    assert_eq!(harness.deck()["history"], serde_json::json!([]));
}

#[test]
fn a_press_with_no_usable_request_id_is_refused_before_anything_starts() {
    // The record is what a repeat is recognised by. A press that cannot be recorded under
    // a name that comes back is refused rather than run and then lost.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let pad = add_pad(
        &harness,
        "Check",
        &format!("printf 'x\\n' >> {}", repo.join("ran").display()),
    );
    for id in ["", "short", &"x".repeat(129), "has spaces in it ok"] {
        let refused = harness.post("/api/terminals/commands", &press(id, &pad, &repo));
        assert_eq!(refused.status, 400, "{id:?}: {}", refused.body);
        assert!(refused.body.contains("request id"), "{}", refused.body);
    }
    assert!(
        !repo.join("ran").exists(),
        "nothing is spawned for a press that was not recorded"
    );
    assert_eq!(harness.sessions(), 0);
    assert_eq!(harness.deck()["history"], serde_json::json!([]));
}

#[test]
fn a_start_that_was_never_recorded_is_retained_accounted_and_not_run_again() {
    // The failure the ordering is for: the press is on paper, the process starts, and the
    // write that was to name its pty does not happen. The process is not killed and not
    // forgotten - its token is on the row - so a repeat is answered with an uncertain
    // outcome rather than running the command a second time, and the exit status still
    // lands on the press it belongs to when it is observed.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let tally = repo.join("ran");
    let pad = add_pad(
        &harness,
        "Check",
        &format!("printf 'x\\n' >> {}; exit 5", tally.display()),
    );
    let body = press("press-unrecorded-000", &pad, &repo);

    let project = harness.project();
    let mut store = harness.store();
    let pad_row = store.terminal_pads(project).unwrap()[0].clone();
    let claim = store
        .claim_terminal_dispatch(
            project,
            &ai_team_core::PressRequest {
                request_id: "press-unrecorded-000",
                worktree: &repo.to_string_lossy(),
                pad: pad_row.id,
                revision: pad_row.revision,
                confirmed: &pad_row.command,
            },
        )
        .unwrap();
    let claimed = match claim {
        ai_team_core::DispatchClaim::Fresh(claimed) => claimed,
        ai_team_core::DispatchClaim::Replay(row) => panic!("expected a fresh claim: {row:?}"),
    };

    // The spawn, through the fixture $SHELL - and then nothing records it.
    let terminals = ai_team_core::Terminals::new();
    let started = terminals
        .start_command(&repo, &claimed.label, &claimed.command, &claimed.token)
        .unwrap();
    let outcome = terminals
        .await_command(&started.token, Duration::from_secs(30))
        .expect("the command runs to the end whether or not anybody wrote it down");
    assert_eq!(outcome.exit_code, Some(5));
    assert_eq!(std::fs::read_to_string(&tally).unwrap(), "x\n");

    // The window tries again. It must not run.
    let again = harness.post("/api/terminals/commands", &body);
    assert_eq!(again.status, 200, "{}", again.body);
    let again = again.json();
    assert_eq!(again["replayed"], true);
    assert_eq!(again["token"], claimed.token);
    assert_eq!(
        again["state"], "uncertain",
        "a press whose start was never written down is not a pass, not a failure, and not \
         grounds for running it again: {again}"
    );
    assert_eq!(
        std::fs::read_to_string(&tally).unwrap(),
        "x\n",
        "the command ran once"
    );

    // The outcome was observed, so it is recorded - on the row its token names.
    assert!(store.settle_terminal_command(&outcome).unwrap());
    let row = harness.row(&claimed.token);
    assert_eq!(row["state"], "finished");
    assert_eq!(row["exit_code"], 5);
    assert_eq!(
        row["dispatch"], "intent",
        "what was recorded is what is said"
    );
}

#[test]
fn a_press_from_an_app_that_is_gone_is_unknown_or_uncertain_rather_than_running() {
    // Rows written by an earlier app session: one that got as far as a pty, one that did
    // not. This process started neither, so it has no outcome for either, and saying so
    // is the whole job - a row stuck on `running` for ever is a build nobody chases.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    let pad = add_pad(&harness, "Check", "true");
    let project = harness.project();
    let mut store = harness.store();
    let pad_row = store.terminal_pads(project).unwrap()[0].clone();
    let claim = |store: &mut ai_team_core::Store, id: &str| match store
        .claim_terminal_dispatch(
            project,
            &ai_team_core::PressRequest {
                request_id: id,
                worktree: &repo.to_string_lossy(),
                pad: pad_row.id,
                revision: pad_row.revision,
                confirmed: &pad_row.command,
            },
        )
        .unwrap()
    {
        ai_team_core::DispatchClaim::Fresh(claimed) => claimed,
        ai_team_core::DispatchClaim::Replay(row) => panic!("expected a fresh claim: {row:?}"),
    };

    let before_the_crash = claim(&mut store, "press-earlier-started");
    store
        .record_terminal_start(&before_the_crash.token, 404)
        .unwrap();
    let during_the_crash = claim(&mut store, "press-earlier-intent0");

    assert_eq!(harness.row(&before_the_crash.token)["state"], "unknown");
    assert_eq!(harness.row(&during_the_crash.token)["state"], "uncertain");
    assert_eq!(
        harness.deck()["running"],
        serde_json::json!([]),
        "this process is measuring neither of them"
    );
    assert_eq!(
        harness.sessions(),
        0,
        "and reading the deck started nothing to make them tidy"
    );
    assert_eq!(pad["revision"], 1);
}

#[test]
fn a_pad_cannot_carry_a_second_command_or_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = checkout(&root);
    let harness = harness(&root, &repo);

    for body in [
        r#"{"project":"widget","label":"Two","command":"true\ntouch second"}"#,
        r#"{"project":"widget","label":"Empty","command":"  "}"#,
        r#"{"project":"widget","label":"  ","command":"true"}"#,
    ] {
        let refused = harness.post("/api/terminals/pads", body);
        assert_eq!(refused.status, 400, "{}", refused.body);
    }
    assert_eq!(
        harness.get("/api/terminals/pads?project=widget").json(),
        serde_json::json!([])
    );
}
