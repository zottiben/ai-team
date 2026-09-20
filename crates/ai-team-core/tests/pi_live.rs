//! One real Pi turn, driven the way ai-team drives a seat (D20).
//!
//! Skipped when `pi` is not on PATH, because ai-team never installs a neighbour (D17) and
//! a machine without it should not fail the suite. On a machine that has it, this is the
//! test that would have caught every framing, deadlock and accounting bug in the runtime
//! - the rest of the Pi tests are about lines of JSON, and this one is about a process.

use std::path::PathBuf;

use ai_team_core::{PiDisposition, PiProcess, PiTurn};

fn have_pi() -> bool {
    std::process::Command::new("pi")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn worktree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "# fixture\n").unwrap();
    dir
}

#[tokio::test]
async fn a_real_turn_streams_events_and_settles() {
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let mut turn = PiTurn::new(
        dir.path(),
        "Reply with exactly the word READY. Do nothing else.",
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let mut kinds = Vec::new();
    let outcome = process
        .drive(|event| kinds.push(event.kind.clone()))
        .await
        .expect("the turn is driven to its end");

    assert!(outcome.settled, "stderr: {}", outcome.stderr);
    assert!(!outcome.failed, "stderr: {}", outcome.stderr);
    assert_eq!(outcome.exit_code, Some(0));

    // The session is what a later turn resumes; without it there is no continuity.
    assert!(outcome.session_id.is_some(), "no session id in the stream");

    // The model's own words, captured from the stream rather than read back out of the
    // event table where ai-team's dispatch notices also live (rule 8).
    let said = outcome.said.expect("the assistant said something");
    assert!(said.to_uppercase().contains("READY"), "{said}");

    // Real accounting, not a fixture: a turn that ran cost something.
    let usage = outcome.usage.expect("usage on the stream");
    assert!(usage.billable() > 0, "{usage:?}");
    assert!(usage.tokens_out > 0, "{usage:?}");

    // And the stream really was a stream.
    assert!(kinds.iter().any(|k| k == "session"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "agent_settled"), "{kinds:?}");
}

#[tokio::test]
async fn a_turn_runs_in_its_lease_and_its_tools_act_there() {
    // D10 on this runtime is an OS fact rather than a check: the child's working
    // directory is the lease, so a relative path a tool writes lands inside it.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let mut turn = PiTurn::new(
        dir.path(),
        "Create a file named PROOF.txt in the current directory containing exactly: inside. \
         Then stop.",
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let outcome = process.drive(|_| {}).await.expect("driven");
    assert!(outcome.settled, "stderr: {}", outcome.stderr);

    let written = dir.path().join("PROOF.txt");
    assert!(written.exists(), "the turn wrote nothing into its lease");
    let body = std::fs::read_to_string(&written).unwrap();
    assert!(body.contains("inside"), "{body}");
}

#[tokio::test]
async fn a_read_only_seat_cannot_write_through_its_tools() {
    // The access guarantee, enforced by Pi rather than by which files ai-team generated.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let mut turn = PiTurn::new(
        dir.path(),
        "Use your write tool to create DENIED.txt in the current directory. If you have no \
         write tool, say NO WRITE TOOL and stop. Do not use bash.",
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());
    turn.exclude_tools = vec!["write".into(), "edit".into()];

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let outcome = process.drive(|_| {}).await.expect("driven");
    assert!(outcome.settled, "stderr: {}", outcome.stderr);
    assert!(
        !dir.path().join("DENIED.txt").exists(),
        "a read-only seat wrote a file through its tools"
    );
}

#[tokio::test]
async fn every_recorded_row_comes_from_a_classified_event() {
    // The seam: many stream events, few rows. If this ever records everything, the event
    // table becomes the transport log rather than the permanent record.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let mut turn = PiTurn::new(dir.path(), "Say OK and stop.");
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let mut seen = 0usize;
    let mut recorded = 0usize;
    process
        .drive(|event| {
            seen += 1;
            if matches!(event.classify(), PiDisposition::Record(_, _)) {
                recorded += 1;
            }
        })
        .await
        .expect("driven");

    assert!(seen > recorded, "seen {seen}, recorded {recorded}");
    assert!(recorded > 0, "nothing was recorded at all");
}

#[tokio::test]
async fn a_missing_lease_fails_before_anything_is_spawned() {
    let turn = PiTurn::new(PathBuf::from("/tmp/ai-team-no-such-lease-8812"), "x");
    assert!(PiProcess::start(&turn).is_err());
}

/// Ask a turn to write `target`, with or without the guard, and say whether it landed.
///
/// The control matters more than the refusal: "the file is not there" is not evidence the
/// guard stopped anything unless the same prompt puts it there when the guard is absent.
async fn tried_to_escape(guarded: bool) -> (bool, String) {
    let dir = worktree();
    let guards = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("ESCAPED.txt");

    let mut turn = PiTurn::new(
        dir.path(),
        format!(
            "Use your write tool to create the file {} containing the word escaped. \
             Report verbatim what the tool returned.",
            target.display()
        ),
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());
    if guarded {
        turn.guard = Some(ai_team_core::install_guard_at(guards.path()).unwrap());
    }

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let outcome = process.drive(|_| {}).await.expect("driven");
    assert!(outcome.settled, "stderr: {}", outcome.stderr);
    (target.exists(), outcome.said.unwrap_or_default())
}

#[tokio::test]
async fn the_guard_refuses_a_write_outside_the_lease() {
    // The guarantee the pivot gave away and the extension gives back: Pi's own `write`
    // answers to nobody until this is loaded.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }

    // Control: without the guard the same prompt really does escape. Without this, a
    // model that simply declined would look exactly like a guard that worked.
    let (escaped, _) = tried_to_escape(false).await;
    assert!(
        escaped,
        "the control did not escape, so the guarded case proves nothing"
    );

    let (escaped, said) = tried_to_escape(true).await;
    assert!(!escaped, "the guard let a turn write outside its lease");
    assert!(
        said.to_lowercase().contains("refus") || said.to_lowercase().contains("outside"),
        "the turn did not report being refused: {said}"
    );
}

#[tokio::test]
async fn the_guard_refuses_to_publish() {
    // A node's work is a draft. `git push` is the ordinary way that stops being true -
    // an agent finishing a task and helpfully pushing it.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let guards = tempfile::tempdir().unwrap();
    let guard = ai_team_core::install_guard_at(guards.path()).unwrap();

    let mut turn = PiTurn::new(
        dir.path(),
        "Run exactly this shell command with your bash tool: git push origin main. \
         Then report verbatim what the tool returned.",
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());
    turn.guard = Some(guard);

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let outcome = process.drive(|_| {}).await.expect("driven");
    assert!(outcome.settled, "stderr: {}", outcome.stderr);

    let said = outcome.said.unwrap_or_default();
    assert!(
        said.to_lowercase().contains("refus") || said.to_lowercase().contains("block"),
        "the turn did not report being refused: {said}"
    );
}

#[tokio::test]
async fn the_guard_leaves_ordinary_work_alone() {
    // A guard that refuses real work is one the operator turns off.
    if !have_pi() {
        eprintln!("skipping: `pi` is not on PATH");
        return;
    }
    let dir = worktree();
    let guards = tempfile::tempdir().unwrap();
    let guard = ai_team_core::install_guard_at(guards.path()).unwrap();

    let mut turn = PiTurn::new(
        dir.path(),
        "Create NOTES.md in the current directory containing the word fine, then run \
         `git status --porcelain` with bash. Then stop.",
    );
    turn.provider = Some("claude-subscription".into());
    turn.model = Some("claude-sonnet-5".into());
    turn.guard = Some(guard);

    let mut process = PiProcess::start(&turn).expect("pi starts");
    let outcome = process.drive(|_| {}).await.expect("driven");
    assert!(outcome.settled, "stderr: {}", outcome.stderr);
    assert!(
        dir.path().join("NOTES.md").exists(),
        "the guard blocked ordinary work in the lease"
    );
}
