use super::*;

#[tokio::test]
async fn malformed_apps_are_refused_before_either_installed_program_changes() {
    for case in [
        "missing-app",
        "cli-as-app",
        "missing-executable",
        "no-executable-name",
        "outside-executable",
    ] {
        let f = Fixture::new();
        let app = f.root.join("payload/ai-team.app");
        match case {
            "missing-app" => fs::remove_dir_all(&app).unwrap(),
            "cli-as-app" => {
                fs::copy(
                    f.root.join("payload/ait"),
                    app.join("Contents/MacOS/ai-team"),
                )
                .unwrap();
            }
            "missing-executable" => fs::remove_file(app.join("Contents/MacOS/ai-team")).unwrap(),
            _ => {
                let path = app.join("Contents/Info.plist");
                let plist = fs::read_to_string(&path).unwrap();
                fs::write(
                    path,
                    plist.replace(
                        "<key>CFBundleExecutable</key><string>ai-team</string>",
                        if case == "no-executable-name" {
                            ""
                        } else {
                            "<key>CFBundleExecutable</key><string>../../ait</string>"
                        },
                    ),
                )
                .unwrap();
            }
        }
        f.package("0.7.6");
        let manager = f.manager();
        let status = manager.check(false).await;
        let result = manager
            .apply("0.7.6", status.approval.as_deref().unwrap())
            .await;
        assert!(result.is_err(), "{case}: {result:?}");
        assert_eq!(
            f.installation.targets().await.unwrap(),
            status.targets,
            "{case}"
        );
        assert!(!f.installation.pending().exists(), "{case}");
        f.protected();
    }
}

#[tokio::test]
async fn an_explicit_approved_update_can_replace_an_app_damaged_by_the_legacy_updater() {
    let f = Fixture::new();
    let app = f.installation.desktop.as_ref().unwrap();
    let target = app.join("Contents/MacOS/ai-team");
    fs::copy(f.installation.cli.as_ref().unwrap(), &target).unwrap();
    let manager = f.manager();
    let status = manager.check(false).await;
    manager
        .apply("0.7.6", status.approval.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(target).unwrap(), "desktop fixture 0.7.6");
    assert_eq!(
        super::super::files::plist(app, "CFBundleShortVersionString").unwrap(),
        "0.7.6"
    );
    f.protected();
}

#[test]
fn sleeps_when_a_parent_test_asks_it_to() {
    if let Some(marker) = std::env::var_os("AI_TEAM_UPDATE_SLEEPER") {
        fs::write(marker, "up").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
}

#[tokio::test]
async fn replacing_a_running_desktop_preserves_its_executing_inode() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new();
    let target = f
        .installation
        .desktop
        .as_ref()
        .unwrap()
        .join("Contents/MacOS/ai-team");
    // Do not copy a system arm64e binary: current macOS kills those outside /bin.
    fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
    let marker = f.root.join("sleeper-up");
    let mut child = Child(
        Command::new(&target)
            .args([
                "update::manager::tests::payload::sleeps_when_a_parent_test_asks_it_to",
                "--exact",
            ])
            .env_clear()
            .env("HOME", &f.root)
            .env("AI_TEAM_UPDATE_SLEEPER", &marker)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !marker.exists() {
        assert!(std::time::Instant::now() < deadline, "child never started");
        assert!(child.0.try_wait().unwrap().is_none(), "child exited early");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    #[cfg(target_os = "linux")]
    assert!(fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&target)
        .is_err());
    let manager = f.manager();
    let status = manager.check(false).await;
    manager
        .apply("0.7.6", status.approval.as_deref().unwrap())
        .await
        .unwrap();
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "updating must not terminate the running desktop"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "desktop fixture 0.7.6");
}

#[tokio::test]
async fn a_payload_cli_symlink_cannot_execute_or_install_a_program_outside_the_archive() {
    let f = Fixture::new();
    let outside = f.root.join("outside-payload");
    executable(&outside, "0.7.6");
    fs::remove_file(f.root.join("payload/ait")).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("payload/ait")).unwrap();
    f.package("0.7.6");
    let manager = f.manager();
    let status = manager.check(false).await;
    let result = manager
        .apply("0.7.6", status.approval.as_deref().unwrap())
        .await;
    assert!(
        result.is_err(),
        "a release program must be its own bytes, not a symlink to another installation"
    );
    assert!(!f.installation.pending().exists());
    assert_eq!(
        super::super::cli_version(f.installation.cli.as_ref().unwrap())
            .await
            .unwrap()
            .as_deref(),
        Some("0.7.4")
    );
}
