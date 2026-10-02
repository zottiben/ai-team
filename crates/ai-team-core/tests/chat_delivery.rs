//! Real disposable Git repositories; fake gh. Never model traffic or live publication.
#![cfg(unix)]

use ai_team_core::{
    chat_changes::{self, DeliveryAction, DraftTarget, Finding},
    *,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    remote: PathBuf,
    store: Store,
    chat: i64,
    other: i64,
    target: DraftTarget,
    base: String,
    commit: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let remote = dir.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@invalid"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        let draft = dir.path().join("draft");
        git(
            &repo,
            &["worktree", "add", "-qb", "draft", draft.to_str().unwrap()],
        );
        std::fs::write(draft.join("README.md"), "verified draft\n").unwrap();
        std::fs::write(draft.join("new.txt"), "new\n").unwrap();
        git(&draft, &["add", "."]);
        git(&draft, &["commit", "-qm", "draft"]);
        let commit = git(&draft, &["rev-parse", "HEAD"]);
        git(&repo, &["worktree", "remove", draft.to_str().unwrap()]);
        git(
            dir.path(),
            &[
                "init",
                "--bare",
                "-q",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        );
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&repo, &["push", "-q", "origin", "main"]);
        let mut store = Store::init(&dir.path().join("team.sqlite")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Delivery".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .seed_default_team(project.id, &RoleModelDefault::local_floor())
            .unwrap();
        let new_chat = || NewChat {
            project_id: project.id,
            workspace: repo.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        };
        let chat = store
            .create_chat_in_mode(new_chat(), ChatMode::Team)
            .unwrap()
            .id;
        let other = store.create_chat(new_chat()).unwrap().id;
        let turn = store
            .begin_chat_turn(chat, "request", "Build", &ModelRegistry::local_only())
            .unwrap();
        // Seed only verified-build evidence. This fixture tests delivery, not the verifier.
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute("UPDATE chat_team_run SET base_sha = ?2, approved_revision = 1, phase = 'finished', quiescent = 1, supervisor_pid = NULL, supervisor_identity = NULL WHERE run_id = ?1", rusqlite::params![turn.run_id, base]).unwrap();
        conn.execute(
            "UPDATE chat SET active_node_id = NULL WHERE id = ?1",
            [chat],
        )
        .unwrap();
        store
            .set_node_status(turn.node_id, NodeStatus::Done)
            .unwrap();
        store.set_run_status(turn.run_id, RunStatus::Done).unwrap();
        conn.execute("INSERT INTO chat_build_slice (run_id,slice_key,planner_slice_id,approved_rev,branch,worktree_path,lease_state,build_status,candidate_sha,commit_sha) VALUES (?1,'S1',1,1,'draft',?2,'released','verified',?3,?3)", rusqlite::params![turn.run_id, draft.to_string_lossy(), commit]).unwrap();
        Self {
            _dir: dir,
            repo,
            remote,
            store,
            chat,
            other,
            target: DraftTarget {
                run_id: turn.run_id,
                slice_key: "S1".into(),
                revision: 1,
            },
            base,
            commit,
        }
    }
    async fn preview(&mut self, action: DeliveryAction) -> chat_changes::Delivery {
        chat_changes::preview(&mut self.store, self.chat, &self.target, action)
            .await
            .unwrap()
    }
    async fn approve(&mut self, delivery: &chat_changes::Delivery) -> chat_changes::Delivery {
        chat_changes::approve(&mut self.store, self.chat, delivery.id, delivery.rev)
            .await
            .unwrap()
    }
}
fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn chat_drafts_have_exact_review_and_separate_idempotent_delivery() {
    // This binary has one test: it alone owns PATH/HOME and the fake gh transport.
    let env = tempfile::tempdir().unwrap();
    std::env::set_var("HOME", env.path());
    std::env::set_var("XDG_CONFIG_HOME", env.path().join("config"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let bin = env.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(
        bin.join("gh"),
        include_str!("fixtures/chat-delivery-gh.mjs"),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    std::env::set_var("DELIVERY_TEST_ROOT", env.path());
    publication(&env).await;
    stale_destinations_and_integration().await;
    abandoned_approval_before_attempt().await;
    ignored_files_and_acknowledgement().await;
    cancelled_publisher(&env).await;
    conservative_boundaries().await;
}

async fn reviewed_dirty_draft() -> Fixture {
    let mut f = Fixture::new();
    std::fs::write(f.repo.join("solo.txt"), "not the draft\n").unwrap();
    std::fs::write(f.repo.join("README.md"), "staged solo\n").unwrap();
    git(&f.repo, &["add", "README.md"]);
    std::fs::write(f.repo.join("README.md"), "unstaged solo\n").unwrap();
    let changes = chat_changes::changes(&mut f.store, f.chat).await.unwrap();
    assert_eq!(changes.head.as_deref(), Some(f.base.as_str()));
    assert_eq!(changes.drafts.len(), 1);
    assert_eq!(changes.staged[0].path, "README.md");
    assert_eq!(changes.unstaged[0].path, "README.md");
    assert_eq!(changes.untracked, ["solo.txt"]);
    assert!(chat_changes::changes(&mut f.store, f.other)
        .await
        .unwrap()
        .drafts
        .is_empty());
    let tree = chat_changes::committed_tree(&mut f.store, f.chat, &f.target)
        .await
        .unwrap();
    assert!(tree.iter().any(|entry| entry.path == "README.md"));
    let request = chat_changes::CommitFileRequest {
        target: f.target.clone(),
        path: "README.md".into(),
    };
    assert_eq!(
        chat_changes::committed_file(&mut f.store, f.chat, &request)
            .await
            .unwrap()
            .text
            .as_deref(),
        Some("verified draft\n")
    );
    assert!(
        chat_changes::committed_file(&mut f.store, f.other, &request)
            .await
            .is_err()
    );
    let missing = chat_changes::CommitFileRequest {
        target: f.target.clone(),
        path: "../../other-checkout/file".into(),
    };
    assert!(chat_changes::committed_file(&mut f.store, f.chat, &missing)
        .await
        .is_err());
    let reviewed = chat_changes::review(&mut f.store, f.chat, &f.target)
        .await
        .unwrap();
    assert_eq!(reviewed.files.len(), 2);
    assert!(reviewed
        .files
        .iter()
        .flat_map(|f| &f.hunks)
        .flat_map(|h| &h.lines)
        .any(|l| l.text == "verified draft"));
    assert!(chat_changes::review(&mut f.store, f.other, &f.target)
        .await
        .is_err());
    let stale = DraftTarget {
        revision: 99,
        ..f.target.clone()
    };
    assert!(chat_changes::review(&mut f.store, f.chat, &stale)
        .await
        .is_err());
    let finding = Finding {
        target: f.target.clone(),
        body: "Please exercise the edge case".into(),
    };
    assert!(f.store.review_chat_draft(f.other, &finding).is_err());
    f.store.review_chat_draft(f.chat, &finding).unwrap();
    assert_eq!(
        chat_changes::review(&mut f.store, f.chat, &f.target)
            .await
            .unwrap()
            .findings
            .len(),
        1
    );
    assert!(
        chat_changes::preview(&mut f.store, f.chat, &f.target, DeliveryAction::Integrate)
            .await
            .unwrap_err()
            .to_string()
            .contains("dirty")
    );
    f
}

async fn publication(env: &tempfile::TempDir) {
    let mut f = reviewed_dirty_draft().await;
    // Repository defaults must not expand an approval into publishing extra tags.
    git(
        &f.repo,
        &[
            "tag",
            "-a",
            "unapproved-tag",
            "-m",
            "not approved",
            &f.commit,
        ],
    );
    git(&f.repo, &["config", "push.followTags", "true"]);
    // Publishing a verified immutable commit must not stage, commit, or include solo dirt.
    let push = f.preview(DeliveryAction::Push).await;
    assert!(
        chat_changes::approve(&mut f.store, f.other, push.id, push.rev)
            .await
            .is_err()
    );
    let pushed = f.approve(&push).await;
    assert_eq!(pushed.state, "done", "{pushed:?}");
    assert!(
        git(&f.remote, &["for-each-ref", "refs/tags"]).is_empty(),
        "approval must not publish tags"
    );
    assert_eq!(
        git(&f.remote, &["rev-parse", &push.snapshot.delivery_branch]),
        f.commit
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), f.base);
    assert_eq!(
        std::fs::read_to_string(f.repo.join("README.md")).unwrap(),
        "unstaged solo\n"
    );
    let events = f.store.events(f.target.run_id, None, 100).unwrap().len();
    assert_eq!(f.approve(&push).await.state, "done");
    assert_eq!(
        f.store.events(f.target.run_id, None, 100).unwrap().len(),
        events
    );
    std::fs::write(env.path().join("gh-head"), &f.commit).unwrap();
    let pr = f.preview(DeliveryAction::PullRequest).await;
    assert!(!env.path().join("gh-created").exists());
    let opened = f.approve(&pr).await;
    assert_eq!(opened.state, "done", "{opened:?}");
    assert_eq!(
        opened.result.as_deref(),
        Some("https://github.com/fixture/delivery/pull/1")
    );
    let log = std::fs::read_to_string(env.path().join("gh-log")).unwrap();
    assert!(log.contains("--draft"));
    assert!(!log.contains("[\"pr\",\"merge\"") && !log.contains("[\"pr\",\"edit\""));
    assert_eq!(f.approve(&pr).await.state, "done");
    assert_eq!(
        std::fs::read_to_string(env.path().join("gh-log")).unwrap(),
        log
    );
}

async fn stale_destinations_and_integration() {
    let mut f = Fixture::new();
    let integrate = f.preview(DeliveryAction::Integrate).await;
    std::fs::write(f.repo.join("solo.txt"), "preserve\n").unwrap();
    assert_eq!(f.approve(&integrate).await.state, "refused");
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), f.base);
    std::fs::remove_file(f.repo.join("solo.txt")).unwrap();
    let integrate = f.preview(DeliveryAction::Integrate).await;
    assert_eq!(f.approve(&integrate).await.state, "done");
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), f.commit);
    assert_eq!(f.approve(&integrate).await.state, "done");

    let mut f = Fixture::new();
    let push = f.preview(DeliveryAction::Push).await;
    git(
        &f.repo,
        &["remote", "set-url", "origin", "/missing-destination"],
    );
    assert_eq!(f.approve(&push).await.state, "refused");
    assert!(git(&f.remote, &["for-each-ref", "refs/heads/ai-team"]).is_empty());
}

async fn abandoned_approval_before_attempt() {
    let mut f = Fixture::new();
    let push = f.preview(DeliveryAction::Push).await;
    // Simulate interruption after admission, without inventing proof of command exit.
    let conn = rusqlite::Connection::open(f.store.path()).unwrap();
    conn.execute(
        "UPDATE chat_delivery SET state = 'running', child_journal = 1, rev = rev + 1 WHERE id = ?1",
        [push.id],
    )
    .unwrap();
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "later",
            "must not run",
            &ModelRegistry::local_only()
        )
        .unwrap_err()
        .to_string()
        .contains("delivery"));
    assert!(
        chat_changes::approve(&mut f.store, f.chat, push.id, push.rev)
            .await
            .is_err()
    );
    assert!(conn
        .execute(
            "UPDATE chat_delivery SET snapshot_json = '{}' WHERE id = ?1",
            [push.id]
        )
        .is_err());
    let interrupted = f.store.chat_delivery(f.chat, push.id).unwrap();
    assert!(
        chat_changes::inspect(&mut f.store, f.other, push.id, interrupted.rev)
            .await
            .is_err()
    );
    let inspected = chat_changes::inspect(&mut f.store, f.chat, push.id, interrupted.rev)
        .await
        .unwrap();
    assert_eq!(inspected.delivery.state, "refused");
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "after-inspection",
            "safe to continue",
            &ModelRegistry::local_only()
        )
        .is_ok());
}

async fn ignored_files_and_acknowledgement() {
    let mut f = Fixture::new();
    let integration = f.preview(DeliveryAction::Integrate).await;
    // Ignored files are allowed at preview, but must never be overwritten by fast-forward.
    std::fs::write(f.repo.join(".git/info/exclude"), "new.txt\n").unwrap();
    std::fs::write(f.repo.join("new.txt"), "keep ignored bytes\n").unwrap();
    assert_eq!(f.approve(&integration).await.state, "refused");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("new.txt")).unwrap(),
        "keep ignored bytes\n"
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), f.base);
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "after-refusal",
            "not stranded",
            &ModelRegistry::local_only()
        )
        .is_ok());

    let mut f = Fixture::new();
    let integration = f.preview(DeliveryAction::Integrate).await;
    let conn = rusqlite::Connection::open(f.store.path()).unwrap();
    conn.execute(
        "UPDATE chat_delivery SET state='inspection', attempted=1, child_journal=1 WHERE id=?1",
        [integration.id],
    )
    .unwrap();
    // An operator changed the branch while the app was down. Keeping it needs its own
    // acknowledgement; neither a failed action nor inspection may certify integration.
    std::fs::write(f.repo.join("manual.txt"), "operator work\n").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-qm", "manual"]);
    let inspection = chat_changes::inspect(&mut f.store, f.chat, integration.id, integration.rev)
        .await
        .unwrap();
    assert_eq!(inspection.delivery.state, "inspection");
    let acknowledgement = chat_changes::DeliveryAcknowledgement {
        delivery_id: integration.id,
        expect_revision: inspection.delivery.rev,
        checkout: inspection.checkout.unwrap(),
        reason: "Keep my manual integration for further review".into(),
    };
    assert!(
        chat_changes::acknowledge(&mut f.store, f.other, &acknowledgement)
            .await
            .is_err()
    );
    let kept = chat_changes::acknowledge(&mut f.store, f.chat, &acknowledgement)
        .await
        .unwrap();
    assert_eq!(kept.delivery.state, "acknowledged");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("manual.txt")).unwrap(),
        "operator work\n"
    );
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "after-acknowledgement",
            "keep working",
            &ModelRegistry::local_only()
        )
        .is_ok());
}

async fn conservative_boundaries() {
    let mut f = Fixture::new();
    let push = f.preview(DeliveryAction::Push).await;
    git(
        &f.remote,
        &[
            "update-ref",
            &format!("refs/heads/{}", push.snapshot.delivery_branch),
            &f.base,
        ],
    );
    assert_eq!(f.approve(&push).await.state, "refused");
    assert_eq!(
        git(&f.remote, &["rev-parse", &push.snapshot.delivery_branch]),
        f.base
    );
    let team = f.store.run(f.target.run_id).unwrap().team_id.unwrap();
    let mut policy = f.store.team(team).unwrap().delivery;
    policy.merge = DeliveryPolicy::Manual;
    f.store.update_delivery(team, policy).unwrap();
    assert!(
        chat_changes::preview(&mut f.store, f.chat, &f.target, DeliveryAction::Integrate)
            .await
            .unwrap_err()
            .to_string()
            .contains("manual")
    );
    git(&f.repo, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    let changes = chat_changes::changes(&mut f.store, f.chat).await.unwrap();
    assert!(changes.head.is_none());
    assert!(!changes.issues.is_empty());
    assert_eq!(changes.drafts.len(), 1);
    assert_eq!(changes.deliveries.len(), 1);

    let mut f = Fixture::new();
    let push = f.preview(DeliveryAction::Push).await;
    assert_eq!(f.approve(&push).await.state, "done");
    let integration = f.preview(DeliveryAction::Integrate).await;
    let conn = rusqlite::Connection::open(f.store.path()).unwrap();
    let boot: String = conn
        .query_row("SELECT boot FROM chat_child LIMIT 1", [], |row| row.get(0))
        .unwrap();
    conn.execute(
        "UPDATE chat_delivery SET state='running', attempted=1, child_journal=1 WHERE id=?1",
        [integration.id],
    )
    .unwrap();
    conn.execute("INSERT INTO chat_child (run_id,epoch,kind,program,boot,created_at) VALUES (?1,1,'command','git',?2,'fixture')", rusqlite::params![f.target.run_id, boot]).unwrap();
    let inspected = chat_changes::inspect(&mut f.store, f.chat, integration.id, integration.rev)
        .await
        .unwrap();
    assert_eq!(inspected.delivery.state, "inspection");
    assert!(inspected.checkout.is_none());
    let forged = chat_changes::DeliveryAcknowledgement {
        delivery_id: integration.id,
        expect_revision: inspected.delivery.rev,
        checkout: chat_changes::CheckoutEvidence {
            head: f.base.clone(),
            branch: Some("main".into()),
            status: String::new(),
        },
        reason: "must not bypass unknown process evidence".into(),
    };
    assert_eq!(
        chat_changes::acknowledge(&mut f.store, f.chat, &forged)
            .await
            .unwrap()
            .delivery
            .state,
        "inspection"
    );
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "still-uncertain",
            "must refuse",
            &ModelRegistry::local_only()
        )
        .is_err());
}

async fn cancelled_publisher(env: &tempfile::TempDir) {
    let mut f = Fixture::new();
    git(
        &f.repo,
        &["remote", "set-url", "origin", "ext::sh -c false"],
    );
    assert!(
        chat_changes::preview(&mut f.store, f.chat, &f.target, DeliveryAction::Push)
            .await
            .unwrap_err()
            .to_string()
            .contains("remote helper")
    );

    // Aborting an approved fake-gh command leaves a journalled process group, not a
    // licence to replay the write. Inspection drains it and proves that no PR appeared.
    let mut f = Fixture::new();
    let push = f.preview(DeliveryAction::Push).await;
    assert_eq!(f.approve(&push).await.state, "done");
    std::fs::remove_file(env.path().join("gh-created")).unwrap();
    std::fs::write(env.path().join("gh-head"), &f.commit).unwrap();
    std::fs::write(env.path().join("gh-hang"), "").unwrap();
    let pr = f.preview(DeliveryAction::PullRequest).await;
    let db = f.store.path().to_path_buf();
    let chat = f.chat;
    let id = pr.id;
    let task = tokio::spawn(async move {
        let mut store = Store::open(&db).unwrap();
        chat_changes::approve(&mut store, chat, id, pr.rev).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !env.path().join("gh-running").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let running = f.store.chat_delivery(f.chat, id).unwrap();
    assert!(
        chat_changes::inspect(&mut f.store, f.chat, id, running.rev)
            .await
            .is_err(),
        "a live receipt must not be reclaimed"
    );
    task.abort();
    let _ = task.await;
    let inspection = chat_changes::inspect(&mut f.store, f.chat, id, running.rev)
        .await
        .unwrap();
    assert_eq!(
        inspection.delivery.state, "inspection",
        "remote absence is not proof that a submitted request cannot finish later: {inspection:?}"
    );
    assert!(!env.path().join("gh-created").exists());
    let kept = chat_changes::DeliveryAcknowledgement {
        delivery_id: id,
        expect_revision: inspection.delivery.rev,
        checkout: inspection.checkout.unwrap(),
        reason: "Keep the drained but externally uncertain outcome".into(),
    };
    assert_eq!(
        chat_changes::acknowledge(&mut f.store, f.chat, &kept)
            .await
            .unwrap()
            .delivery
            .state,
        "acknowledged"
    );
    assert!(f
        .store
        .begin_chat_turn(
            f.other,
            "after-draining",
            "continue",
            &ModelRegistry::local_only()
        )
        .is_ok());
}
