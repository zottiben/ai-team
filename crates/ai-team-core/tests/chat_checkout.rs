//! Actual checkout/index/ref boundaries, with only scratch Git and fake GitHub.
#![cfg(unix)]
use ai_team_core::{
    chat_changes::checkout as c, ModelRegistry, NewChat, NewProject, NewRepo, NodeStatus, Provider,
    Reasoning, Store,
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
async fn act(store: &mut Store, chat: i64, action: c::Action) -> c::Operation {
    let state = c::state(store, chat).await.unwrap();
    let preview = c::preview(store, chat, &state.fingerprint, action)
        .await
        .unwrap();
    let result = c::approve(store, chat, preview.id, preview.rev)
        .await
        .unwrap();
    assert_eq!(result.state, "done", "{result:?}");
    result
}
#[tokio::test]
async fn exact_checkout_actions_preserve_work_and_never_create_verified_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::env::set_var("HOME", &root);
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git(&repo, &["config", "core.hooksPath", "/dev/null"]);
    let mut store = Store::init(&root.join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "checkout fixture".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .attach_repo(
            project.id,
            NewRepo {
                main_path: Some(repo.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .unwrap();
    let create = || NewChat {
        project_id: project.id,
        workspace: repo.clone(),
        provider: Provider::Local,
        model: "fixture".into(),
        reasoning: Reasoning::High,
    };
    let chat = store.create_chat(create()).unwrap().id;
    let other = store.create_chat(create()).unwrap().id;
    staging(&mut store, &repo, chat, other).await;
    feedback(&mut store, &repo, chat, other).await;
    publication(&mut store, &root, &repo, chat).await;
    uncertainty(&mut store, &repo, chat, other).await;
    fake_github(&root, &repo, &mut store, chat).await;
    git(&repo, &["checkout", "--orphan", "fresh"]);
    assert!(c::state(&mut store, chat).await.unwrap().head.is_none());
    let large = std::fs::File::create(repo.join("large-output")).unwrap();
    large.set_len(33 * 1024 * 1024).unwrap();
    assert!(c::state(&mut store, chat)
        .await
        .unwrap_err()
        .to_string()
        .contains("exact-review limit"));
}
async fn staging(store: &mut Store, repo: &Path, chat: i64, other: i64) {
    std::fs::write(repo.join("README.md"), "first\n").unwrap();
    let before = c::state(store, chat).await.unwrap();
    assert!(before.head.is_none());
    let preview = c::preview(
        store,
        chat,
        &before.fingerprint,
        c::Action::Stage {
            path: "README.md".into(),
        },
    )
    .await
    .unwrap();
    assert!(store.checkout_operation(other, preview.id).is_err());
    std::fs::write(repo.join("README.md"), "new bytes\n").unwrap();
    assert_eq!(
        c::approve(store, chat, preview.id, preview.rev)
            .await
            .unwrap()
            .state,
        "refused"
    );
    assert!(git(repo, &["ls-files"]).is_empty());
    act(
        store,
        chat,
        c::Action::Stage {
            path: "README.md".into(),
        },
    )
    .await;
    act(
        store,
        chat,
        c::Action::Unstage {
            path: "README.md".into(),
        },
    )
    .await;
    assert!(repo.join("README.md").exists());
    act(
        store,
        chat,
        c::Action::Stage {
            path: "README.md".into(),
        },
    )
    .await;
    std::fs::write(repo.join("not-staged"), "keep\n").unwrap();
    act(
        store,
        chat,
        c::Action::Commit {
            message: "operator commit".into(),
        },
    )
    .await;
    assert_eq!(git(repo, &["ls-tree", "--name-only", "HEAD"]), "README.md");
    assert!(repo.join("not-staged").exists());
}
async fn feedback(store: &mut Store, repo: &Path, chat: i64, other: i64) {
    std::fs::write(repo.join("README.md"), "reviewed change\n").unwrap();
    let state = c::state(store, chat).await.unwrap();
    let finding = c::Finding {
        id: 0,
        fingerprint: state.fingerprint,
        head: state.head,
        area: "unstaged".into(),
        path: "README.md".into(),
        side: "new".into(),
        line: 1,
        body: "Handle the error case".into(),
        created_at: String::new(),
    };
    c::finding(store, chat, &finding).await.unwrap();
    assert_eq!(store.checkout_findings(chat).unwrap().len(), 1);
    assert!(store.checkout_findings(other).unwrap().is_empty());
    std::fs::write(repo.join("README.md"), "different line\n").unwrap();
    assert!(c::finding(store, chat, &finding).await.is_err());
    let turn = store
        .begin_chat_turn(other, "busy", "busy", &ModelRegistry::local_only())
        .unwrap();
    let state = c::state(store, chat).await.unwrap();
    assert!(c::preview(
        store,
        chat,
        &state.fingerprint,
        c::Action::Stage {
            path: "README.md".into()
        }
    )
    .await
    .is_err());
    store
        .finish_chat_turn(other, turn.node_id, NodeStatus::Done, None)
        .unwrap();
    act(
        store,
        chat,
        c::Action::Stage {
            path: "README.md".into(),
        },
    )
    .await;
    act(
        store,
        chat,
        c::Action::Commit {
            message: "reviewed change".into(),
        },
    )
    .await;
}
async fn publication(store: &mut Store, root: &Path, repo: &Path, chat: i64) {
    let remote = root.join("remote.git");
    git(repo, &["init", "--bare", remote.to_str().unwrap()]);
    git(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(repo, &["push", "origin", "HEAD:refs/heads/main"]);
    let head = git(repo, &["rev-parse", "HEAD"]);
    let publication = act(store, chat, c::Action::Push).await;
    let branch = &publication.snapshot.remote.as_ref().unwrap().branch;
    assert_eq!(
        git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]),
        head
    );
    // No team row, draft, model turn or auto-integration has been manufactured.
    assert!(ai_team_core::chat_changes::changes(store, chat)
        .await
        .unwrap()
        .drafts
        .is_empty());
    assert!(store.chat_turns(chat).unwrap().is_empty());
    assert_eq!(
        c::approve(store, chat, publication.id, publication.rev)
            .await
            .unwrap()
            .state,
        "done"
    );
}
async fn uncertainty(store: &mut Store, repo: &Path, chat: i64, other: i64) {
    std::fs::write(repo.join("README.md"), "yet another edit\n").unwrap();
    let state = c::state(store, chat).await.unwrap();
    let preview = c::preview(
        store,
        chat,
        &state.fingerprint,
        c::Action::Stage {
            path: "README.md".into(),
        },
    )
    .await
    .unwrap();
    std::fs::write(repo.join(".git/index.lock"), "fixture lock").unwrap();
    let failed = c::approve(store, chat, preview.id, preview.rev)
        .await
        .unwrap();
    assert_eq!(failed.state, "inspection");
    assert!(store
        .begin_chat_turn(other, "blocked", "blocked", &ModelRegistry::local_only())
        .is_err());
    std::fs::remove_file(repo.join(".git/index.lock")).unwrap();
    let inspected = c::inspect(store, chat, failed.id, failed.rev)
        .await
        .unwrap();
    let ack = c::acknowledge(
        store,
        chat,
        failed.id,
        inspected.operation.rev,
        &inspected.checkout.fingerprint,
        "I inspected and kept the index and files",
    )
    .await
    .unwrap();
    assert_eq!(ack.state, "acknowledged");
    assert!(git(repo, &["diff", "--cached", "--name-only"]).is_empty());
}
async fn fake_github(root: &Path, repo: &Path, store: &mut Store, chat: i64) {
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("gh");
    std::fs::write(&script, r#"#!/bin/sh
case "$1 $2" in
'repo view') echo '{"url":"https://github.com/fixture/repo","defaultBranchRef":{"name":"main"}}';;
'pr list')
 if [ -f "$CHECKOUT_TEST_PR" ]; then
   printf '[{"url":"https://github.com/fixture/repo/pull/1","state":"OPEN","baseRefName":"main","headRefOid":"%s","isDraft":true}]' "$(git rev-parse HEAD)"
 else echo '[]'; fi;;
'pr create') touch "$CHECKOUT_TEST_PR"; echo https://github.com/fixture/repo/pull/1;;
*) exit 90;;
esac
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    std::env::set_var("CHECKOUT_TEST_PR", root.join("created-pr"));
    let head = git(repo, &["rev-parse", "HEAD"]);
    let result = act(store, chat, c::Action::PullRequest).await;
    assert_eq!(
        result.result.as_deref(),
        Some("https://github.com/fixture/repo/pull/1")
    );
    assert_eq!(git(repo, &["rev-parse", "HEAD"]), head);
}
