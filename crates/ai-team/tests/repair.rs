#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn opening_a_legacy_broken_app_never_installs_or_restarts_without_approval() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let app = root.join("ai-team.app/Contents/MacOS");
    let bin = root.join("bin");
    let state = root.join("state");
    for path in [&app, &bin, &state, &root.join("home")] {
        fs::create_dir_all(path).unwrap();
    }
    let executable = app.join("ai-team");
    fs::copy(env!("CARGO_BIN_EXE_ait"), &executable).unwrap();
    fs::write(state.join("install-method"), "release\n").unwrap();
    let calls = root.join("calls");
    for tool in ["curl", "open", "osascript", "notify-send"] {
        let path = bin.join(tool);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{tool}' >> '{}'\nexit 99\n",
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(&executable)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("AI_TEAM_HOME", &state)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("SHELL", "/bin/false")
        .env("TMPDIR", root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let calls = fs::read_to_string(calls).unwrap_or_default();
    assert!(
        !calls.lines().any(|line| matches!(line, "curl" | "open")),
        "an app launch is not approval to install or restart: {calls}"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("reinstall"), "{error}");
    assert_eq!(
        fs::read(executable).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_ait")).unwrap()
    );
}
