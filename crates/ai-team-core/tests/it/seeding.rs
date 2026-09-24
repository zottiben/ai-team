//! Seeding a team asks nothing of the machine it runs on.
//!
//! Which models a new team gets is a question about this machine - its profile, and the
//! catalogue Pi lists for it - and it used to be asked inside the store. So every test
//! that seeded a team ran `pi` a few times over: nearly two seconds each, forty-odd
//! tests, most of the unit suite's time on a machine with Pi installed. It also meant
//! the rosters under test were whatever that machine allowed, and CI, with no Pi and no
//! profile, tested a different team from the laptop.
//!
//! Checked in a child process rather than here, because the machine this sets up - a
//! profile that allows Claude, and a `pi` that writes down every call - is environment,
//! and changing the environment of a test binary that is running other tests is a race.

use std::process::Command;

use ai_team_core::{NewProject, Provider, RoleModelDefault, Store};

const CALLS_ENV: &str = "AI_TEAM_TEST_PI_CALLS";

/// A `pi` that answers the way a machine signed in to Claude would, and writes down that
/// it was asked.
const RECORDING_PI: &str = r#"#!/bin/sh
echo "pi $*" >> "$AI_TEAM_TEST_PI_CALLS"
case "$*" in
  *--list-models*)
    echo "provider             model          context  max-out  thinking  images"
    echo "claude-subscription  claude-opus-5  1M       128K     yes       yes"
    ;;
  *auth*) echo '{"status":"ready","authType":"oauth"}' ;;
esac
"#;

const CLAUDE_ALLOWED: &str = r#"version = 1
fallback = ["claude", "openai", "zai", "local"]

[providers]
claude = true
openai = false
zai = false
local = true
"#;

#[test]
fn seeding_a_team_runs_no_process_and_reads_no_profile() {
    use std::os::unix::fs::PermissionsExt as _;

    let machine = tempfile::tempdir().unwrap();
    let bin = machine.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pi = bin.join("pi");
    std::fs::write(&pi, RECORDING_PI).unwrap();
    std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = machine.path().join("config");
    std::fs::create_dir_all(config.join("ai-team")).unwrap();
    std::fs::write(config.join("ai-team/machine.toml"), CLAUDE_ALLOWED).unwrap();
    let calls = machine.path().join("calls");

    let path = std::env::var("PATH").unwrap_or_default();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["seeding::seed_a_team", "--exact", "--ignored"])
        .env("PATH", format!("{}:{path}", bin.display()))
        .env("XDG_CONFIG_HOME", &config)
        .env(CALLS_ENV, &calls)
        .output()
        .unwrap();

    assert!(
        !calls.exists(),
        "seeding a team ran Pi:\n{}",
        std::fs::read_to_string(&calls).unwrap_or_default()
    );
    assert!(
        child.status.success(),
        "seeding failed:\n{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

/// The seeding itself, run by the test above in the machine it set up.
#[test]
#[ignore = "run by seeding_a_team_runs_no_process_and_reads_no_profile"]
fn seed_a_team() {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    let team = store
        .seed_default_team(project.id, &RoleModelDefault::local_floor())
        .unwrap();

    for agent in store.agents(team.id).unwrap() {
        assert_eq!(
            (agent.provider, agent.model.as_str()),
            (Provider::Local, "auto"),
            "{} was seeded from the machine rather than from what it was given",
            agent.role
        );
    }
}
