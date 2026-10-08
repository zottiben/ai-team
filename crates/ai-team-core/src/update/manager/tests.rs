use super::*;
use crate::{NewChat, NewProject, Provider, Reasoning, Store};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

mod payload;
mod recovery;
mod terminals;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    installation: UpdateInstallation,
    store: Store,
    chat: i64,
}

fn future_version() -> String {
    format!(
        "{}.0.0",
        semver::Version::parse(crate::current_version())
            .unwrap()
            .major
            + 1
    )
}

fn executable(path: &Path, version: &str) {
    fs::write(path, format!("#!/bin/sh\nprintf 'ait {version}\\n'\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn app(path: &Path, version: &str) {
    fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    fs::write(path.join("Contents/Info.plist"), format!("<?xml version=\"1.0\"?><plist><dict><key>CFBundleIdentifier</key><string>dev.zottiben.ai-team</string><key>CFBundleExecutable</key><string>ai-team</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>")).unwrap();
    fs::write(
        path.join("Contents/MacOS/ai-team"),
        format!("desktop fixture {version}"),
    )
    .unwrap();
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for name in [
            "bin", "home", "state", "config", "releases", "repo", "payload",
        ] {
            fs::create_dir(root.join(name)).unwrap();
        }
        executable(&root.join("bin/ait"), "0.7.4");
        app(&root.join("ai-team.app"), "0.7.4");
        fs::write(root.join("config/machine.toml"), "preserve configuration").unwrap();
        fs::create_dir(root.join("state/oauth")).unwrap();
        fs::write(
            root.join("state/oauth/fixture.json"),
            "preserve fixture credential",
        )
        .unwrap();
        fs::write(root.join("state/install-method"), "source\n").unwrap();
        let db = root.join("state/team.db");
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Preserve this project".into(),
                ..Default::default()
            })
            .unwrap();
        let chat = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: root.join("repo"),
                provider: Provider::Local,
                model: "test-model".into(),
                reasoning: Reasoning::High,
            })
            .unwrap();
        store
            .change_chat_plan(
                chat.id,
                crate::planning::PlanActor::Human,
                crate::planning::PlanAction::CreatePlan {
                    expect_revision: 0,
                    title: "Preserve this plan".into(),
                    summary: None,
                },
            )
            .unwrap();
        let installation = UpdateInstallation {
            host: Host::Desktop,
            cli: Some(root.join("bin/ait")),
            desktop: Some(root.join("ai-team.app")),
            home: root.join("home"),
            data_dir: root.join("state"),
            database: Some(db),
            config_dir: Some(root.join("config")),
            method: Method::Source,
            blocked: None,
        };
        let fixture = Self {
            _dir: dir,
            root,
            installation,
            store,
            chat: chat.id,
        };
        fixture.release("0.7.6");
        fixture
    }

    fn release(&self, version: &str) {
        let payload = self.root.join("payload");
        executable(&payload.join("ait"), version);
        app(&payload.join("ai-team.app"), version);
        self.package(version);
    }

    fn package(&self, version: &str) {
        let payload = self.root.join("payload");
        let name = format!("ai-team-v{version}-macos-universal.tar.gz");
        let archive = self.root.join("releases").join(&name);
        assert!(Command::new("tar")
            .arg("czf")
            .arg(&archive)
            .arg("-C")
            .arg(payload)
            .arg(".")
            .status()
            .unwrap()
            .success());
        fs::write(
            self.root.join("releases/checksums.txt"),
            format!(
                "{}  {name}\n",
                super::super::sha256(&fs::read(archive).unwrap())
            ),
        )
        .unwrap();
        fs::write(self.root.join("releases/latest"), version).unwrap();
    }

    fn manager(&self) -> UpdateManager {
        UpdateManager::isolated(self.installation.clone(), &self.root).unwrap()
    }

    fn protected(&self) {
        assert_eq!(
            fs::read_to_string(self.root.join("config/machine.toml")).unwrap(),
            "preserve configuration"
        );
        assert_eq!(
            fs::read_to_string(self.root.join("state/oauth/fixture.json")).unwrap(),
            "preserve fixture credential"
        );
        assert_eq!(self.store.projects().unwrap().len(), 1);
        assert_eq!(self.store.chat_turns(self.chat).unwrap().len(), 0);
        assert_eq!(
            self.store
                .chat_plan(self.chat, crate::planning::PlanActor::Human)
                .unwrap()
                .bundle
                .unwrap()
                .plan
                .title,
            "Preserve this plan"
        );
    }
}

#[tokio::test]
async fn checks_are_read_only_and_paired_updates_preserve_data_auth_and_running_inodes() {
    let mut f = Fixture::new();
    // Keep the simulated upgrade newer than this test process across release bumps.
    let version = future_version();
    f.release(&version);
    let before = f.installation.targets().await.unwrap();
    let manager = f.manager();
    let checked = manager.check(false).await;
    assert!(checked.can_update, "{:?}", checked.blocked);
    assert_eq!(checked.targets, before);
    assert!(!f.root.join("state/updates").exists());
    assert!(!f.installation.pending().exists());
    let old_inode = f.root.join("running-cli");
    fs::hard_link(f.installation.cli.as_ref().unwrap(), &old_inode).unwrap();
    let updated = manager
        .apply(&version, checked.approval.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        super::cli_version(&old_inode).await.unwrap().as_deref(),
        Some("0.7.4")
    );
    assert_eq!(
        super::cli_version(f.installation.cli.as_ref().unwrap())
            .await
            .unwrap()
            .as_deref(),
        Some(version.as_str())
    );
    assert_eq!(
        files::plist(
            f.installation.desktop.as_ref().unwrap(),
            "CFBundleShortVersionString"
        )
        .unwrap(),
        version
    );
    assert_eq!(updated.targets, f.installation.targets().await.unwrap());
    assert!(updated.backup.join("team.db").exists());
    assert!(updated.backup.join("team.db.planning.sqlite").exists());
    assert_eq!(
        files::fingerprint(&updated.backup.join("program-0")).unwrap(),
        before[0].fingerprint
    );
    assert_eq!(
        files::fingerprint(&updated.backup.join("program-1")).unwrap(),
        before[1].fingerprint
    );
    assert_eq!(
        fs::read_to_string(f.root.join("state/install-method")).unwrap(),
        "release\n"
    );
    assert!(updated.warnings.is_empty(), "{:?}", updated.warnings);
    assert!(!updated.backup.join("payload").exists());
    assert!(!fs::read_dir(&f.root).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".ai-team-update-")));
    f.protected();
    assert!(f
        .store
        .rename_chat(f.chat, "old process must not write")
        .unwrap_err()
        .to_string()
        .contains("restart"));
    let status = manager.check(false).await;
    assert_eq!(status.state, UpdateState::Restart);
    assert!(!status.can_update);
}

#[tokio::test]
async fn checksum_failure_cannot_install_or_fall_back_to_a_source_build() {
    let f = Fixture::new();
    let before = f.installation.targets().await.unwrap();
    fs::remove_file(f.root.join("releases/checksums.txt")).unwrap();
    let manager = f.manager();
    let checked = manager.check(false).await;
    assert!(manager
        .apply("0.7.6", checked.approval.as_deref().unwrap())
        .await
        .is_err());
    assert_eq!(f.installation.targets().await.unwrap(), before);
    assert!(!f.installation.pending().exists());
    assert_eq!(
        fs::read_to_string(f.root.join("state/install-method")).unwrap(),
        "source\n"
    );
    f.protected();
}

#[tokio::test]
async fn changed_release_or_destination_requires_a_new_approval() {
    let f = Fixture::new();
    let manager = f.manager();
    let checked = manager.check(false).await;
    f.release("0.7.7");
    assert!(manager
        .apply("0.7.6", checked.approval.as_deref().unwrap())
        .await
        .unwrap_err()
        .to_string()
        .contains("changed"));
    assert!(!f.root.join("state/updates").exists());
    let manager = f.manager();
    let checked = manager.check(false).await;
    executable(f.installation.cli.as_ref().unwrap(), "0.7.3");
    assert!(manager
        .apply("0.7.7", checked.approval.as_deref().unwrap())
        .await
        .is_err());
    assert!(!f.root.join("state/updates").exists());
}

#[tokio::test]
async fn setup_and_cross_process_fences_prevent_installation_without_stopping_work() {
    let mut f = Fixture::new();
    let manager = f.manager();
    let checked = manager.check(false).await;
    let lock = super::super::fence::open(&f.installation.data_dir).unwrap();
    lock.try_lock().unwrap();
    assert!(f.store.rename_chat(f.chat, "not admitted").is_err());
    assert!(Store::open(f.installation.database.as_ref().unwrap()).is_err());
    assert!(manager
        .apply("0.7.6", checked.approval.as_deref().unwrap())
        .await
        .is_err());
    drop(lock);
    let chat = f.store.chat(f.chat).unwrap();
    f.store
        .request_workspace_setup(chat.project_id, &f.root.join("repo"), "setup", "branch")
        .unwrap();
    assert!(manager
        .apply("0.7.6", checked.approval.as_deref().unwrap())
        .await
        .unwrap_err()
        .to_string()
        .contains("finish or inspect"));
    assert!(!f.root.join("state/updates").exists());
}

#[tokio::test]
async fn isolated_update_configuration_cannot_reach_neighbouring_installations() {
    let mut f = Fixture::new();
    let neighbour = tempfile::tempdir().unwrap();
    f.installation.cli = Some(neighbour.path().join("ait"));
    assert!(UpdateManager::isolated(f.installation, &f.root).is_err());
}
