use super::*;

fn interrupted(f: &mut Fixture) -> PathBuf {
    let version = fs::read_to_string(f.root.join("releases/latest")).unwrap();
    let backup = f.root.join("state/updates/interrupted");
    fs::create_dir_all(&backup).unwrap();
    assert!(f
        .store
        .install_application_update(&version, &backup, || Err::<(), _>(Error::invalid(
            "simulated interruption after snapshots"
        )))
        .is_err());
    let targets = tokio::runtime::Handle::current()
        .block_on(f.installation.targets())
        .unwrap();
    let next: Vec<_> = targets
        .iter()
        .map(|target| {
            let payload = f.root.join("payload").join(if target.name == "CLI" {
                "ait"
            } else {
                "ai-team.app"
            });
            let mut next = target.clone();
            next.version = Some(version.clone());
            next.fingerprint = files::fingerprint(&payload).unwrap();
            next
        })
        .collect();
    files::record(&backup.join("prepared.json"), &next).unwrap();
    files::record(
        &f.installation.pending(),
        &serde_json::json!({"version":version, "backup":backup, "targets":targets}),
    )
    .unwrap();
    backup
}

#[tokio::test(flavor = "multi_thread")]
async fn inspection_distinguishes_unchanged_mixed_and_fully_installed_bytes_without_replay() {
    let version = future_version();
    let mut f = Fixture::new();
    f.release(&version);
    let manager = f.manager();
    tokio::task::block_in_place(|| interrupted(&mut f));
    assert!(f.store.rename_chat(f.chat, "not admitted").is_err());
    let status = manager.check(false).await;
    assert_eq!(status.state, UpdateState::Inspection);
    assert!(!status.can_update);
    assert!(!manager.inspect().await.unwrap().installed);
    assert!(!f.installation.pending().exists());
    f.store.rename_chat(f.chat, "still here").unwrap();

    // A second independent fixture models death between the two program renames.
    let mut f = Fixture::new();
    f.release(&version);
    let manager = f.manager();
    tokio::task::block_in_place(|| interrupted(&mut f));
    fs::rename(
        f.installation.desktop.as_ref().unwrap(),
        f.root.join("old.app"),
    )
    .unwrap();
    files::copy(
        &f.root.join("payload/ai-team.app"),
        f.installation.desktop.as_ref().unwrap(),
    )
    .unwrap();
    assert!(manager
        .inspect()
        .await
        .unwrap_err()
        .to_string()
        .contains("mixed"));
    assert!(f.installation.pending().exists());
    assert!(
        Store::installed_application_version(f.installation.database.as_ref().unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        super::super::cli_version(f.installation.cli.as_ref().unwrap())
            .await
            .unwrap()
            .as_deref(),
        Some("0.7.4")
    );

    // Matching prepared bytes can reconcile a lost database commit; no installer runs.
    files::copy(
        &f.root.join("payload/ait"),
        f.installation.cli.as_ref().unwrap(),
    )
    .unwrap();
    fs::remove_dir_all(f.root.join("releases")).unwrap();
    assert!(manager.inspect().await.unwrap().installed);
    assert!(!f.installation.pending().exists());
    assert_eq!(
        Store::installed_application_version(f.installation.database.as_ref().unwrap())
            .unwrap()
            .as_deref(),
        Some(version.as_str())
    );
    assert!(f
        .store
        .rename_chat(f.chat, "old process must restart")
        .is_err());
    f.protected();
}

#[tokio::test]
async fn desktop_only_update_does_not_create_a_cli() {
    let mut f = Fixture::new();
    fs::remove_file(f.installation.cli.take().unwrap()).unwrap();
    let manager = f.manager();
    let status = manager.check(false).await;
    assert_eq!(status.targets.len(), 1);
    assert_eq!(status.targets[0].name, "Desktop app");
    assert!(manager
        .apply("0.7.6", status.approval.as_deref().unwrap())
        .await
        .unwrap()
        .warnings
        .is_empty());
    assert!(!f.root.join("bin/ait").exists());
    let error = manager.admit().unwrap_err().to_string();
    assert!(error.contains("restart"), "{error}");
    f.protected();
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_keeps_installation_fenced_and_preparation_does_not_leave_a_pending_receipt() {
    let f = Fixture::new();
    let cli = f.root.join("payload/ait");
    fs::write(&cli, format!("#!/bin/sh\n[ ! -f '{}' ] || exit 71\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep .01; done\nprintf 'ait 0.7.6\\n'\n", f.installation.pending().display(), f.root.join("probe-started").display(), f.root.join("probe-release").display())).unwrap();
    f.package("0.7.6");
    let manager = f.manager();
    let status = manager.check(false).await;
    let approval = status.approval.unwrap();
    let pending = {
        let manager = manager.clone();
        let approval = approval.clone();
        tokio::spawn(async move { manager.apply("0.7.6", &approval).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f.root.join("probe-started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!f.installation.pending().exists());
    assert!(Store::open(f.installation.database.as_ref().unwrap()).is_err());
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_eq!(manager.check(false).await.state, UpdateState::Updating);
    assert!(manager.apply("0.7.6", &approval).await.is_err());
    fs::write(f.root.join("probe-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while Store::installed_application_version(f.installation.database.as_ref().unwrap())
            .unwrap()
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        while manager.busy() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!f.installation.pending().exists());
    f.protected();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_brief_ordinary_write_is_not_misreported_as_a_second_updater() {
    let f = Fixture::new();
    let lock = super::super::super::fence::shared(f.installation.database.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let manager = f.manager();
    let status = manager.check(false).await;
    let worker = tokio::spawn(async move {
        manager
            .apply("0.7.6", status.approval.as_deref().unwrap())
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(lock);
    assert!(worker.await.unwrap().is_ok());
}
