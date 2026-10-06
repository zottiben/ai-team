//! Actual offline Pi process resumes in the chosen Git worktree, never the old session.
#![cfg(unix)]
use ai_team_core::planning::PlanActor;
use ai_team_core::{
    drive_chat, ModelRegistry, NewChat, NewProject, NewRepo, Provider, Reasoning, Store,
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};
fn git(path: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(path)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=/dev/null"
        ])
        .args(args)
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
    root: std::path::PathBuf,
    repo: std::path::PathBuf,
    linked: std::path::PathBuf,
    store: Store,
    chat: ai_team_core::Chat,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let linked = root.join("linked");
    let bin = root.join("bin");
    let config = root.join("config/ai-team");
    for dir in [&repo, &bin, &config, &root.join("home")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("AI_TEAM_HOME", root.join("state"));
    std::env::set_var("CHAT_TEST_ROOT", &root);
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    std::fs::write(
        config.join("machine.toml"),
        ai_team_core::DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
    )
    .unwrap();
    executable(&bin.join("pi"), include_str!("fixtures/chat-pi.sh"));
    for name in [
        "aip",
        "claude",
        "codex",
        "security",
        "gh",
        "osascript",
        "notify-send",
    ] {
        executable(&bin.join(name), "#!/bin/sh\nexit 99\n");
    }
    executable(
        &bin.join("awt"),
        "#!/bin/sh\n[ \"$*\" = 'status --json' ] || exit 99\nprintf '%s' '{\"worktrees\":[]}'\n",
    );
    std::fs::write(root.join("mode"), "done").unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "keep\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    git(
        &repo,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let mut store = Store::init(&root.join("team.db")).unwrap();
    let p = store
        .create_project(NewProject {
            name: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .attach_repo(
            p.id,
            NewRepo {
                main_path: Some(repo.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: p.id,
            workspace: repo.clone(),
            provider: Provider::Local,
            model: "fixture-model".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    Fixture {
        _temp: temp,
        root,
        repo,
        linked,
        store,
        chat,
    }
}

#[tokio::test]
async fn a_confirmed_switch_drives_the_next_process_in_the_new_checkout() {
    let Fixture {
        _temp,
        root,
        repo,
        linked,
        mut store,
        chat,
    } = fixture();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &ModelRegistry::local_only())
        .unwrap();
    drive_chat(store.path(), chat.id, first.node_id, false)
        .await
        .unwrap();
    let before = std::fs::read(repo.join("changed.txt")).unwrap();
    let target = ai_team_core::chat_workspaces::validate(&repo, &linked)
        .await
        .unwrap();
    let request = store
        .request_chat_workspace(chat.id, PlanActor::Human, &target)
        .unwrap();
    store
        .apply_chat_workspace(
            chat.id,
            request.id,
            store.chat(chat.id).unwrap().rev,
            &target,
        )
        .unwrap();
    let next = store
        .begin_chat_turn(chat.id, "second", "second", &ModelRegistry::local_only())
        .unwrap();
    assert!(store.node_run(next.node_id).unwrap().session_id.is_none());
    drive_chat(store.path(), chat.id, next.node_id, false)
        .await
        .unwrap();
    assert_eq!(std::fs::read(repo.join("changed.txt")).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(linked.join("changed.txt")).unwrap(),
        "kept\n"
    );
    let back = ai_team_core::chat_workspaces::validate(&repo, &repo)
        .await
        .unwrap();
    let request = store
        .request_chat_workspace(chat.id, PlanActor::Human, &back)
        .unwrap();
    store
        .apply_chat_workspace(chat.id, request.id, store.chat(chat.id).unwrap().rev, &back)
        .unwrap();
    let third = store
        .begin_chat_turn(chat.id, "third", "third", &ModelRegistry::local_only())
        .unwrap();
    assert!(
        store.node_run(third.node_id).unwrap().session_id.is_none(),
        "returning to an old checkout still requires a fresh session"
    );
    drive_chat(store.path(), chat.id, third.node_id, false)
        .await
        .unwrap();
    let calls = std::fs::read_to_string(root.join("calls")).unwrap();
    let calls: Vec<_> = calls
        .lines()
        .map(|line| line.split('|').collect::<Vec<_>>())
        .collect();
    assert_eq!(calls.len(), 3);
    assert_ne!(calls[0][0], calls[1][0]);
    assert_ne!(calls[0][0], calls[2][0]);
    assert_eq!(calls[2][1], repo.to_string_lossy());
    assert_eq!(calls[0][1], repo.to_string_lossy());
    assert_eq!(calls[1][1], linked.to_string_lossy());
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 3);
}
