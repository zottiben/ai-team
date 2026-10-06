//! Real scratch Git and child processes; AWT/package managers are offline fixtures.
#![cfg(unix)]
use ai_team_core::{
    workspace_setup::{self, Setup},
    ModelRegistry, NewChat, NewProject, NewRepo, NodeStatus, Provider, Reasoning, Store,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn git(cwd: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null"
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .current_dir(cwd)
        .status()
        .unwrap()
        .success());
}
fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    store: Store,
    project: i64,
    chat: i64,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let bin = root.join("bin");
    for dir in [&repo, &bin, &root.join("home"), &root.join("config")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (key, path) in [
        ("HOME", root.join("home")),
        ("XDG_CONFIG_HOME", root.join("config")),
        ("AI_TEAM_HOME", root.join("state")),
        ("WORKSPACE_FIXTURE", root.clone()),
    ] {
        std::env::set_var(key, path);
    }
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    tools(&bin);
    git(&repo, &["init", "-q"]);
    for (path, body) in [
        ("README.md", "preserve"),
        ("composer.json", "{}"),
        ("composer.lock", "{}"),
        ("package.json", "{\"name\":\"fixture\"}"),
        ("bun.lock", "fixture"),
        (".gitignore", "vendor/\nnode_modules/\n"),
    ] {
        std::fs::write(repo.join(path), body).unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let mut store = Store::init(&root.join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "fixture".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    store
        .attach_repo(
            project,
            NewRepo {
                main_path: Some(repo.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project,
            workspace: repo.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap()
        .id;
    Fixture {
        _temp: temp,
        root,
        repo,
        store,
        project,
        chat,
    }
}
fn tools(bin: &Path) {
    for name in [
        "pi",
        "aip",
        "claude",
        "codex",
        "gh",
        "security",
        "osascript",
        "notify-send",
    ] {
        executable(&bin.join(name), "#!/bin/sh\nexit 99\n");
    }
    executable(
        &bin.join("awt"),
        r#"#!/bin/sh
set -eu
r="$WORKSPACE_FIXTURE"
if [ "$*" = 'status --json' ]; then
 printf '{"worktrees":['; separator=''
 for file in "$r"/holder-*; do
  [ -f "$file" ] || continue
  n="${file##*-}"; holder=$(cat "$file")
  printf '%s{"name":"%s","path":"%s/lease%s","status":"leased","leaseHolder":"%s","processes":[]}' "$separator" "$n" "$r" "$n" "$holder"; separator=,
 done
 printf ']}'; exit 0
fi
[ "$1 $2 $3" = 'get --lease --lease-holder' ] || exit 99
n=0; [ ! -f "$r/count" ] || n=$(cat "$r/count"); n=$((n+1)); printf '%s' "$n" > "$r/count"
git worktree add --detach "$r/lease$n" >/dev/null
printf '%s' "$4" > "$r/holder-$n"
mkdir -p "$r/lease$n/vendor" "$r/lease$n/node_modules"
printf 'stale' > "$r/lease$n/vendor/autoload.php"
printf 'AWT hook ran' > "$r/lease$n/hook-result"
[ ! -f "$r/missing-lock" ] || rm "$r/lease$n/bun.lock"
printf '%s/lease%s\n' "$r" "$n"
"#,
    );
    executable(
        &bin.join("composer"),
        r#"#!/bin/sh
set -eu
[ "$1" = install ] && [ -f composer.lock ]
printf 'composer:%s:%s\n' "$PWD" "$*" >> "$WORKSPACE_FIXTURE/trace"
mkdir -p vendor; printf 'current' > vendor/autoload.php
"#,
    );
    executable(
        &bin.join("bun"),
        r#"#!/bin/sh
set -eu
[ "$*" = 'install --frozen-lockfile' ] && [ "$(cat vendor/autoload.php)" = current ]
printf 'bun:%s:%s\n' "$PWD" "$*" >> "$WORKSPACE_FIXTURE/trace"
[ ! -f "$WORKSPACE_FIXTURE/fail-bun" ] || { echo 'fixture install refused' >&2; exit 17; }
mkdir -p node_modules; printf 'current' > node_modules/fixture
"#,
    );
}
async fn acquire(f: &mut Fixture) -> Setup {
    let (first, started) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "new-checkout", "")
        .unwrap();
    assert!(started);
    let (replay, started) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "new-checkout", "")
        .unwrap();
    assert!(!started);
    assert_eq!(first.id, replay.id);
    assert!(f
        .store
        .request_workspace_setup(f.project, &f.repo, "new-checkout", "different")
        .is_err());
    assert!(f
        .store
        .begin_chat_turn(f.chat, "must wait", "wait", &ModelRegistry::local_only())
        .unwrap_err()
        .to_string()
        .contains("AWT setup"));
    let ready = workspace_setup::run(f.store.path(), f.project, first.id)
        .await
        .unwrap();
    assert_eq!(ready.state, "ready", "{}", ready.detail);
    workspace_setup::run(f.store.path(), f.project, first.id)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.root.join("count")).unwrap(),
        "1",
        "a repeated start must not acquire again"
    );
    ready
}
async fn ready_lease(f: &Fixture, ready: &Setup) {
    let path = f.root.join("lease1");
    assert_eq!(
        ready.workspace_path.as_deref(),
        Some(path.to_str().unwrap())
    );
    for (file, body) in [
        ("hook-result", "AWT hook ran"),
        ("vendor/autoload.php", "current"),
        ("node_modules/fixture", "current"),
    ] {
        assert_eq!(std::fs::read_to_string(path.join(file)).unwrap(), body);
    }
    assert!(ready
        .steps
        .iter()
        .any(|step| step.command.starts_with("composer install")));
    assert!(ready
        .steps
        .iter()
        .any(|step| step.command == "bun install --frozen-lockfile"));
    let choices = ai_team_core::chat_workspaces::owned_choices(f.store.path(), &f.repo)
        .await
        .unwrap();
    assert!(choices
        .iter()
        .find(|choice| choice.path == path.to_string_lossy())
        .unwrap()
        .unavailable
        .is_none());
    let alias = f.root.join("state-alias");
    std::os::unix::fs::symlink(&f.root, &alias).unwrap();
    let choices = ai_team_core::chat_workspaces::owned_choices(&alias.join("team.db"), &f.repo)
        .await
        .unwrap();
    assert!(
        choices
            .iter()
            .find(|choice| choice.path == path.to_string_lossy())
            .unwrap()
            .unavailable
            .is_none(),
        "a filesystem alias must not make this database's own setup look foreign"
    );
    let foreign = Store::init(&f.root.join("other.db")).unwrap();
    let choices = ai_team_core::chat_workspaces::owned_choices(foreign.path(), &f.repo)
        .await
        .unwrap();
    assert!(
        choices
            .iter()
            .find(|choice| choice.path == path.to_string_lossy())
            .unwrap()
            .unavailable
            .is_some(),
        "a different app database cannot adopt a lease by name alone"
    );
}
async fn retry(f: &mut Fixture) {
    let turn = f
        .store
        .begin_chat_turn(
            f.chat,
            "allowed now",
            "after-setup",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    f.store
        .finish_chat_turn(f.chat, turn.node_id, NodeStatus::Cancelled, None)
        .unwrap();
    std::fs::write(f.root.join("fail-bun"), "").unwrap();
    let (second, _) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "second-checkout", "feature/setup-test")
        .unwrap();
    let failed = workspace_setup::run(f.store.path(), f.project, second.id)
        .await
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.detail.contains("fixture install refused"));
    assert!(f.root.join("holder-2").exists());
    assert!(f.root.join("lease2/vendor/autoload.php").exists());
    assert!(failed.steps.iter().any(|step| !step.passed));
    let failed = inspection_cannot_retarget(f, &failed).await;
    std::fs::remove_file(f.root.join("fail-bun")).unwrap();
    // Recovery through an alias must use the same canonical database-scoped holder.
    let again = workspace_setup::retry_dependencies(
        &f.root.join("state-alias/team.db"),
        f.project,
        second.id,
        failed.rev,
    )
    .await
    .unwrap();
    assert_eq!(again.state, "ready", "{}", again.detail);
    assert_eq!(
        std::fs::read_to_string(f.root.join("count")).unwrap(),
        "2",
        "dependency retry must not get/reset another checkout"
    );
    assert!(
        again.steps.iter().any(|step| !step.passed),
        "earlier failure evidence survives retry"
    );
}
async fn inspection_cannot_retarget(f: &Fixture, failed: &Setup) -> Setup {
    let elsewhere = f.root.join("lease99");
    git(
        &f.repo,
        &["worktree", "add", "--detach", elsewhere.to_str().unwrap()],
    );
    let held = std::fs::read(f.root.join("holder-2")).unwrap();
    std::fs::remove_file(f.root.join("holder-2")).unwrap();
    std::fs::write(f.root.join("holder-99"), &held).unwrap();
    let inspected = workspace_setup::inspect(f.store.path(), f.project, failed.id, failed.rev)
        .await
        .unwrap();
    assert_eq!(
        inspected.state, "inspection",
        "a holder label moving must not redirect recorded setup evidence"
    );
    assert_eq!(inspected.workspace_path, failed.workspace_path);
    std::fs::remove_file(f.root.join("holder-99")).unwrap();
    std::fs::write(f.root.join("holder-2"), held).unwrap();
    workspace_setup::inspect(f.store.path(), f.project, failed.id, inspected.rev)
        .await
        .unwrap()
}
async fn inspect_and_missing_lock(f: &mut Fixture) {
    let (abandoned, _) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "lost-before-spawn", "")
        .unwrap();
    let inspected =
        workspace_setup::inspect(f.store.path(), f.project, abandoned.id, abandoned.rev)
            .await
            .unwrap();
    assert_eq!(inspected.state, "failed");
    assert!(inspected.workspace_path.is_none());
    assert_eq!(std::fs::read_to_string(f.root.join("count")).unwrap(), "2");
    std::fs::write(f.root.join("missing-lock"), "").unwrap();
    let (receipt, _) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "missing-lock", "")
        .unwrap();
    let failed = workspace_setup::run(f.store.path(), f.project, receipt.id)
        .await
        .unwrap();
    assert_eq!(
        failed.state, "failed",
        "a missing JS lock must not become false readiness: {}",
        failed.detail
    );
    assert!(failed.detail.contains("no supported lockfile"));
    assert!(f.root.join("holder-3").exists());
    std::fs::remove_file(f.root.join("missing-lock")).unwrap();
    std::fs::copy(f.repo.join("bun.lock"), f.root.join("lease3/bun.lock")).unwrap();
    let ready =
        workspace_setup::retry_dependencies(f.store.path(), f.project, failed.id, failed.rev)
            .await
            .unwrap();
    assert_eq!(ready.state, "ready", "{}", ready.detail);
    assert_eq!(std::fs::read_to_string(f.root.join("count")).unwrap(), "3");
}
async fn active_work_still_fences_an_unleased_pool_slot(f: &mut Fixture) {
    let other = f
        .store
        .create_project(NewProject {
            name: "other registration".into(),
            ..Default::default()
        })
        .unwrap();
    f.store
        .attach_repo(
            other.id,
            NewRepo {
                main_path: Some(f.repo.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let chat = f
        .store
        .create_chat(NewChat {
            project_id: other.id,
            workspace: f.root.join("lease3"),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let turn = f
        .store
        .begin_chat_turn(
            chat.id,
            "reserved, not spawned",
            "reserved-slot",
            &ModelRegistry::local_only(),
        )
        .unwrap();
    // This slot has live app authority even in the gap before Pi is spawned, and
    // through another registration. AWT's process scan cannot see that reservation.
    std::fs::write(f.root.join("holder-3"), "").unwrap();
    let (request, _) = f
        .store
        .request_workspace_setup(f.project, &f.repo, "protect-existing-work", "")
        .unwrap();
    let result = workspace_setup::run(f.store.path(), f.project, request.id)
        .await
        .unwrap();
    assert_eq!(
        result.state, "inspection",
        "unleased reserved work must stop acquisition: {}",
        result.detail
    );
    assert!(result.detail.contains("active or retained"));
    assert_eq!(
        std::fs::read_to_string(f.root.join("count")).unwrap(),
        "3",
        "the refusal must precede any awt get"
    );
    f.store
        .finish_chat_turn(chat.id, turn.node_id, NodeStatus::Cancelled, None)
        .unwrap();
}

// One test owns the process environment. Phases share only this scratch repository.
#[tokio::test]
async fn awt_setup_is_idempotent_bootstraps_warm_dependencies_and_keeps_failed_leases() {
    let mut f = fixture();
    std::env::set_var("GIT_DIR", f.root.join("not-this-checkout"));
    std::env::set_var("GIT_WORK_TREE", f.root.join("also-not-this-checkout"));
    let ready = acquire(&mut f).await;
    ready_lease(&f, &ready).await;
    retry(&mut f).await;
    inspect_and_missing_lock(&mut f).await;
    active_work_still_fences_an_unleased_pool_slot(&mut f).await;
    assert_eq!(
        std::fs::read_to_string(f.repo.join("README.md")).unwrap(),
        "preserve"
    );
    assert!(!f.repo.join("vendor").exists());
    assert!(!f.repo.join("node_modules").exists());
    assert_eq!(
        f.store.chat_turns(f.chat).unwrap().len(),
        1,
        "setup never manufactures an agent turn"
    );
}
