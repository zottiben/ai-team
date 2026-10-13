use super::*;

#[test]
fn terminal_admission_survives_the_request_and_ends_only_after_the_process_and_pipes() {
    let f = Fixture::new();
    let manager = f.manager();
    let terminals = crate::Terminals::new();
    let permit = manager.admit().unwrap();
    let id = terminals.open_program(&f.root, "/bin/cat", &[]).unwrap();
    terminals.protect_update(id, permit).unwrap();
    let lock = super::super::super::fence::open(&f.installation.data_dir).unwrap();
    assert!(lock.try_lock().is_err());
    terminals.write(id, "\u{4}").unwrap();
    let started = Instant::now();
    while lock.try_lock().is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "finished terminal retained its update permit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(terminals.read(id, 0).unwrap().done);
    terminals.close(id).unwrap();
}

#[tokio::test]
async fn an_approved_update_closes_its_sessions_and_retains_scrollback_and_data() {
    let f = Fixture::new();
    let manager = f.manager();
    let terminals = Arc::new(crate::Terminals::new());
    let id = terminals.open_program(&f.root, "/bin/cat", &[]).unwrap();
    terminals
        .protect_update(id, manager.admit().unwrap())
        .unwrap();
    let status = manager.check(true).await;
    let review = terminals.review_update();
    assert_eq!(review.sessions.len(), 1);
    assert!(
        !terminals.read(id, 0).unwrap().done,
        "checks must not stop a session"
    );
    let error = manager
        .apply_with_terminals(
            "0.7.6",
            status.approval.as_deref().unwrap(),
            terminals.clone(),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Close sessions and update"),
        "{error}"
    );
    assert!(!terminals.read(id, 0).unwrap().done);
    manager
        .apply_with_terminals(
            "0.7.6",
            status.approval.as_deref().unwrap(),
            terminals.clone(),
            Some(review.approval),
        )
        .await
        .unwrap();
    assert!(terminals.read(id, 0).unwrap().done);
    assert!(terminals.review_update().sessions.is_empty());
    assert!(!f.installation.pending().exists());
    f.protected();
}

#[tokio::test]
async fn fresh_sessions_foreign_scopes_and_changed_targets_are_never_closed() {
    let f = Fixture::new();
    let manager = f.manager();
    let terminals = Arc::new(crate::Terminals::new());
    let old = terminals.review_update().approval;
    let id = terminals.open_program(&f.root, "/bin/cat", &[]).unwrap();
    terminals
        .protect_update(id, manager.admit().unwrap())
        .unwrap();
    let status = manager.check(true).await;
    let foreign = crate::Terminals::new();
    let other = foreign.open_program(&f.root, "/bin/cat", &[]).unwrap();
    assert_eq!(other, id, "session IDs repeat across app processes");
    let foreign_approval = foreign.review_update().approval;
    foreign.close(other).unwrap();
    for approval in [old, foreign_approval] {
        let error = manager
            .apply_with_terminals(
                "0.7.6",
                status.approval.as_deref().unwrap(),
                terminals.clone(),
                Some(approval),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("sessions changed"), "{error}");
        assert!(!terminals.read(id, 0).unwrap().done);
    }
    let error = manager
        .apply_with_terminals(
            "0.7.6",
            "stale destinations",
            terminals.clone(),
            Some(terminals.review_update().approval),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
    assert!(!terminals.read(id, 0).unwrap().done);
    assert!(!f.root.join("state/updates").exists());
    terminals.close(id).unwrap();
}

#[tokio::test]
async fn active_work_is_explained_before_any_session_is_closed() {
    let mut f = Fixture::new();
    let manager = f.manager();
    let terminals = Arc::new(crate::Terminals::new());
    let id = terminals.open_program(&f.root, "/bin/cat", &[]).unwrap();
    terminals
        .protect_update(id, manager.admit().unwrap())
        .unwrap();
    let status = manager.check(true).await;
    let chat = f.store.chat(f.chat).unwrap();
    f.store
        .request_workspace_setup(chat.project_id, &f.root.join("repo"), "setup", "branch")
        .unwrap();
    let busy = manager.check(true).await;
    assert!(!busy.can_update);
    assert!(busy.blocked.unwrap().contains("workspace setup"));
    let error = manager
        .apply_with_terminals(
            "0.7.6",
            status.approval.as_deref().unwrap(),
            terminals.clone(),
            Some(terminals.review_update().approval),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("workspace setup"), "{error}");
    assert!(!terminals.read(id, 0).unwrap().done);
    assert!(!f.root.join("state/updates").exists());
    terminals.close(id).unwrap();
}

#[tokio::test]
async fn disconnect_during_session_drain_keeps_preparation_reserved_until_install_finishes() {
    struct Release(PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            fs::write(&self.0, "release fixture child").unwrap();
        }
    }
    let f = Fixture::new();
    let release = Release(f.root.join("release"));
    let manager = f.manager();
    let terminals = Arc::new(crate::Terminals::new());
    let id = terminals.open_program(&f.root, "/bin/sh", &["-c".into(), "/bin/sh -c 'trap \"\" HUP; printf held; while [ ! -f release ]; do sleep .01; done' & wait".into()]).unwrap();
    terminals
        .protect_update(id, manager.admit().unwrap())
        .unwrap();
    let started = Instant::now();
    while !terminals.read(id, 0).unwrap().text.contains("held") {
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let approval = manager.check(true).await.approval.unwrap();
    let pending = {
        let manager = manager.clone();
        let terminals = terminals.clone();
        let close = terminals.review_update().approval;
        tokio::spawn(async move {
            manager
                .apply_with_terminals("0.7.6", &approval, terminals, Some(close))
                .await
        })
    };
    let started = Instant::now();
    while !terminals.read(id, 0).unwrap().done {
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(manager
        .admit_terminal()
        .unwrap_err()
        .to_string()
        .contains("preparing"));
    assert!(!f.installation.pending().exists());
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert!(manager.busy());
    assert!(manager.admit_terminal().is_err());
    assert!(
        manager.admit().is_ok(),
        "Stop must remain admissible during preparation"
    );
    let lock = super::super::super::fence::open(&f.installation.data_dir).unwrap();
    assert!(
        lock.try_lock().is_err(),
        "inherited terminal output still holds its permit"
    );
    drop(release);
    let started = Instant::now();
    while manager.busy() {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(terminals.review_update().sessions.is_empty());
    assert_eq!(
        Store::installed_application_version(f.installation.database.as_ref().unwrap())
            .unwrap()
            .as_deref(),
        Some("0.7.6")
    );
    f.protected();
}

#[test]
fn the_update_fence_excludes_an_independent_writer_process() {
    let f = Fixture::new();
    let lock = super::super::super::fence::open(&f.installation.data_dir).unwrap();
    lock.try_lock().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "update::manager::tests::terminals::writer_process_fixture",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &f.root)
        .env(
            "AI_TEAM_UPDATE_TEST_DB",
            f.installation.database.as_ref().unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("independent writer refused"));
}

#[test]
fn writer_process_fixture() {
    let Some(path) = std::env::var_os("AI_TEAM_UPDATE_TEST_DB") else {
        return;
    };
    assert!(Store::open(&PathBuf::from(path))
        .unwrap_err()
        .to_string()
        .contains("updating"));
    println!("independent writer refused");
}
