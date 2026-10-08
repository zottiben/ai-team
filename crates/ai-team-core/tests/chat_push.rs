//! Explicit human chat push, end to end: real disposable Git and a local bare remote.
//! No model traffic, no GitHub, no operator state.
#![cfg(unix)]

use ai_team_core::{
    chat_changes::checkout,
    chat_push::{self, Authority, Intent},
    ChatMode, DeliveryPolicy, DeliverySettings, ModelRegistry, NewChat, NewProject, NewRepo,
    NodeStatus, Provider, Reasoning, RoleModelDefault, RunStatus, Store,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const ASKED: &str = "commit and push and ill open the PR";
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    repo: PathBuf,
    remote: PathBuf,
    store: Store,
    project: i64,
    team: i64,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::env::set_var("HOME", &root);
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@invalid"]);
        git(&repo, &["config", "core.hooksPath", "/dev/null"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        git(
            &root,
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
        let db = root.join("team.sqlite");
        let mut store = Store::init(&db).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Push".into(),
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
        let team = store
            .seed_default_team(project.id, &RoleModelDefault::local_floor())
            .unwrap();
        Self {
            _dir: dir,
            db,
            repo,
            remote,
            store,
            project: project.id,
            team: team.id,
        }
    }

    fn chat(&mut self, mode: ChatMode) -> i64 {
        self.store
            .create_chat_in_mode(
                NewChat {
                    project_id: self.project,
                    workspace: self.repo.clone(),
                    provider: Provider::Local,
                    model: "fixture".into(),
                    reasoning: Reasoning::High,
                },
                mode,
            )
            .unwrap()
            .id
    }

    /// The whole direct-send path a person goes through, minus the HTTP frame.
    async fn send(&mut self, chat: i64, message: &str, request: &str) -> (Authority, Option<i64>) {
        let authority = chat_push::prepare(&mut self.store, chat, message)
            .await
            .unwrap();
        if let Authority::Team(target, _) = &authority {
            chat_push::publish_team(&mut self.store, chat, request, message, target)
                .await
                .unwrap();
            return (authority, None);
        }
        let turn = self
            .store
            .begin_chat_turn(chat, message, request, &ModelRegistry::local_only())
            .unwrap();
        if let Authority::Solo(target) = &authority {
            chat_push::mint(
                &mut self.store,
                chat,
                request,
                message,
                Some(turn.node_id),
                target,
                None,
            )
            .unwrap();
        }
        (authority, Some(turn.node_id))
    }

    /// What the seat does: commit the work, then pin it. Never a push.
    fn agent_commits(&self, file: &str, body: &str, message: &str) -> String {
        std::fs::write(self.repo.join(file), body).unwrap();
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-qm", message]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn turn_ends(&mut self, chat: i64, node: i64) {
        self.store
            .finish_chat_turn(chat, node, NodeStatus::Done, None)
            .unwrap();
    }

    fn remote_sha(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(&self.remote)
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
    }

    fn notes(&self, chat: i64) -> Vec<String> {
        self.store
            .chat_events(chat, 0, 500)
            .unwrap()
            .into_iter()
            .map(|event| event.summary)
            .collect()
    }
}

/// The requirement itself: one direct human message, one branch published by the host.
#[tokio::test]
async fn an_explicit_human_message_publishes_the_working_branch_and_nothing_else() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);

    // Before the person asks, the seat has no authority and cannot invent one.
    let early = f
        .store
        .begin_chat_turn(chat, "add a test", "warm", &ModelRegistry::local_only())
        .unwrap();
    assert!(f
        .store
        .chat_push_grant_for_node(chat, early.node_id)
        .unwrap()
        .is_none());
    let unasked = f.agent_commits("early.txt", "early\n", "unasked work");
    assert!(
        chat_push::request_publication(&f.db, chat, early.node_id, &unasked)
            .await
            .unwrap_err()
            .to_string()
            .contains("has not asked for a push")
    );
    f.turn_ends(chat, early.node_id);
    chat_push::settle(&f.db, chat, early.node_id, false)
        .await
        .unwrap();
    assert_eq!(f.remote_sha("work"), None, "nothing is pushed unasked");

    // Now the person asks, in the words the requirement names.
    let (authority, node) = f.send(chat, ASKED, "ask-1").await;
    let node = node.unwrap();
    assert!(matches!(&authority, Authority::Solo(target) if target.allow_commit));
    assert!(
        f.store
            .chat_push_grant_for_node(chat, node)
            .unwrap()
            .is_some(),
        "the turn carries the authority, so its seat is offered the host tool"
    );

    let commit = f.agent_commits("feature.txt", "done\n", "the asked-for work");
    let answer = chat_push::request_publication(&f.db, chat, node, &commit)
        .await
        .unwrap();
    assert!(answer.contains("after this turn ends"), "{answer}");
    assert_eq!(
        f.remote_sha("work"),
        None,
        "pinning is not publishing; the turn is still live"
    );

    f.turn_ends(chat, node);
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work").as_deref(), Some(commit.as_str()));

    let receipts = f.store.checkout_operations(chat).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].state, "done");
    assert!(matches!(
        receipts[0].snapshot.action,
        checkout::Action::PushBranch { .. }
    ));
    // Push permission is not PR permission: no GitHub repository was ever resolved.
    assert!(receipts[0]
        .snapshot
        .remote
        .as_ref()
        .is_some_and(|remote| remote.repository.is_none() && remote.branch == "work"));
    assert!(
        f.notes(chat).contains(&"Pushed".to_string()),
        "the host's own result is in the conversation: {:?}",
        f.notes(chat)
    );

    // Replaying the settle, and the request, publishes nothing a second time.
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    let (replay, _) = f.send(chat, ASKED, "ask-1").await;
    assert!(matches!(replay, Authority::Solo(_)));
    assert_eq!(f.store.checkout_operations(chat).unwrap().len(), 1);
    let grant = f
        .store
        .chat_push_grant_for_request(chat, "ask-1")
        .unwrap()
        .unwrap();
    assert_eq!(grant.state, "spent");
}

/// Everything that must not become authority, in the same real chat.
#[tokio::test]
async fn authority_is_refused_rather_than_guessed() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);

    // Mentioning publishing is a question, not an instruction.
    assert_eq!(chat_push::classify("did you push that?"), Intent::Ambiguous);
    let (asked, node) = f.send(chat, "did you push that?", "ask-vague").await;
    assert!(matches!(asked, Authority::Ask(_)));
    assert!(f
        .store
        .chat_push_grant_for_node(chat, node.unwrap())
        .unwrap()
        .is_none());
    f.turn_ends(chat, node.unwrap());

    // A bare push authorises what is already committed, never a commit made after it.
    let (authority, node) = f.send(chat, "push", "ask-bare").await;
    let node = node.unwrap();
    assert!(matches!(&authority, Authority::Solo(target) if !target.allow_commit));
    let extra = f.agent_commits("extra.txt", "extra\n", "not asked for");
    assert!(chat_push::request_publication(&f.db, chat, node, &extra)
        .await
        .unwrap_err()
        .to_string()
        .contains("not a new commit"));

    // Stopping the turn takes the authority with it.
    f.store.request_chat_stop(chat, node).unwrap();
    assert!(f
        .store
        .chat_push_grant_for_node(chat, node)
        .unwrap()
        .is_none());
    f.turn_ends(chat, node);
    assert!(
        f.store
            .chat_push_grant_for_node(chat, node)
            .unwrap()
            .is_none(),
        "clearing Stop during settlement must never revive publication authority"
    );
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work"), None);

    // Protected names are refused by name, whatever the remote calls its default.
    for protected in ["main", "develop"] {
        git(&f.repo, &["checkout", "-qB", protected]);
        assert!(chat_push::prepare(&mut f.store, chat, ASKED)
            .await
            .unwrap_err()
            .to_string()
            .contains("protected branch name"));
    }
    // A default branch is refused even when its name says nothing.
    git(&f.repo, &["checkout", "-qB", "shipping"]);
    git(&f.repo, &["push", "-q", "origin", "shipping"]);
    git(&f.remote, &["symbolic-ref", "HEAD", "refs/heads/shipping"]);
    assert!(chat_push::prepare(&mut f.store, chat, ASKED)
        .await
        .unwrap_err()
        .to_string()
        .contains("default branch"));
    git(&f.remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // Manual delivery policy is a veto, said when the person asks.
    git(&f.repo, &["checkout", "-q", "work"]);
    f.store
        .update_delivery(
            f.team,
            DeliverySettings {
                push: DeliveryPolicy::Manual,
                ..DeliverySettings::default()
            },
        )
        .unwrap();
    assert!(chat_push::prepare(&mut f.store, chat, ASKED)
        .await
        .unwrap_err()
        .to_string()
        .contains("manual"));
    f.store
        .update_delivery(f.team, DeliverySettings::default())
        .unwrap();

    // The review panel cannot construct a working-branch push for itself.
    let state = checkout::state(&mut f.store, chat).await.unwrap();
    assert!(checkout::preview(
        &mut f.store,
        chat,
        &state.fingerprint,
        checkout::Action::PushBranch {
            branch: "work".into(),
            commit: state.head.clone().unwrap(),
        },
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("your own message"));
}

/// A branch somebody else moved is not overwritten, and the refusal is recorded.
#[tokio::test]
async fn a_diverged_remote_branch_is_never_overwritten() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);
    let (_, node) = f.send(chat, ASKED, "ask-1").await;
    let node = node.unwrap();
    let mine = f.agent_commits("feature.txt", "mine\n", "mine");
    chat_push::request_publication(&f.db, chat, node, &mine)
        .await
        .unwrap();

    // Somebody publishes other work on that branch while the turn runs.
    let theirs = git(&f.repo, &["rev-parse", "HEAD~1"]);
    git(
        &f.repo,
        &[
            "push",
            "-q",
            f.remote.to_str().unwrap(),
            &format!("{theirs}:refs/heads/work"),
        ],
    );
    git(&f.repo, &["commit", "-q", "--allow-empty", "-m", "diverge"]);
    let rewritten = git(&f.repo, &["rev-parse", "HEAD"]);
    git(
        &f.repo,
        &[
            "push",
            "-q",
            "--force",
            f.remote.to_str().unwrap(),
            &format!("{rewritten}:refs/heads/work"),
        ],
    );
    git(&f.repo, &["reset", "-q", "--hard", &mine]);

    f.turn_ends(chat, node);
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work").as_deref(), Some(rewritten.as_str()));
    assert!(f.notes(chat).contains(&"Push refused".to_string()));
    assert_eq!(
        f.store
            .chat_push_grant_for_request(chat, "ask-1")
            .unwrap()
            .unwrap()
            .state,
        "expired"
    );
}

/// Team mode publishes one exact owned draft, and never starts planning instead.
#[tokio::test]
async fn a_team_push_targets_one_exact_owned_draft() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    let chat = f.chat(ChatMode::Team);
    let base = git(&f.repo, &["rev-parse", "HEAD"]);
    let turn = f
        .store
        .begin_chat_turn(chat, "build it", "build", &ModelRegistry::local_only())
        .unwrap();
    let lease = f.repo.parent().unwrap().join("draft");
    git(
        &f.repo,
        &["worktree", "add", "-qb", "draft", lease.to_str().unwrap()],
    );
    std::fs::write(lease.join("new.txt"), "draft\n").unwrap();
    git(&lease, &["add", "."]);
    git(&lease, &["commit", "-qm", "draft"]);
    let commit = git(&lease, &["rev-parse", "HEAD"]);

    // Seed only verified-build evidence; this is about delivery, not the verifier.
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute("UPDATE chat_team_run SET base_sha=?2, approved_revision=1, phase='finished', quiescent=1, supervisor_pid=NULL, supervisor_identity=NULL WHERE run_id=?1", rusqlite::params![turn.run_id, base]).unwrap();
    conn.execute("UPDATE chat SET active_node_id=NULL WHERE id=?1", [chat])
        .unwrap();
    f.store
        .set_node_status(turn.node_id, NodeStatus::Done)
        .unwrap();
    f.store
        .set_run_status(turn.run_id, RunStatus::Done)
        .unwrap();
    let slice = |key: &str| {
        conn.execute("INSERT INTO chat_build_slice (run_id,slice_key,planner_slice_id,approved_rev,branch,worktree_path,lease_state,build_status,candidate_sha,commit_sha) VALUES (?1,?2,?5,1,'draft',?3,'released','verified',?4,?4)",
            rusqlite::params![turn.run_id, key, lease.to_string_lossy(), commit, if key == "S1" { 1 } else { 2 }]).unwrap();
    };

    // With nothing verified, the message is refused rather than turned into planning.
    let empty = chat_push::prepare(&mut f.store, chat, ASKED)
        .await
        .unwrap_err()
        .to_string();
    assert!(empty.contains("no verified draft"), "{empty}");
    assert!(empty.contains("no planning was started"), "{empty}");

    slice("S1");
    let (authority, node) = f.send(chat, ASKED, "ask-team").await;
    assert!(node.is_none(), "a team push starts no turn of its own");
    assert!(matches!(&authority, Authority::Team(target, draft)
        if target.branch == "draft" && target.head_sha == commit && draft.slice_key == "S1"));
    assert_eq!(f.remote_sha("draft").as_deref(), Some(commit.as_str()));
    assert!(f.notes(chat).contains(&"Pushed".to_string()));
    assert_eq!(
        f.store.chat_turns(chat).unwrap().len(),
        1,
        "no planning run"
    );

    // Replaying the same request republishes nothing.
    f.send(chat, ASKED, "ask-team").await;
    assert_eq!(f.store.checkout_operations(chat).unwrap().len(), 1);

    // A second verified draft makes the target ambiguous, so it is asked about.
    slice("S2");
    let several = chat_push::prepare(&mut f.store, chat, ASKED)
        .await
        .unwrap_err()
        .to_string();
    assert!(several.contains("2 verified drafts"), "{several}");
    assert!(several.contains("exact one in Changes"), "{several}");
}

/// A grant belongs to the process the person was talking to, and to the checkout they saw.
#[tokio::test]
async fn a_restart_or_a_checkout_handoff_does_not_revive_a_grant() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);
    let (_, node) = f.send(chat, ASKED, "ask-1").await;
    let node = node.unwrap();
    let commit = f.agent_commits("feature.txt", "done\n", "work");
    chat_push::request_publication(&f.db, chat, node, &commit)
        .await
        .unwrap();
    f.turn_ends(chat, node);

    // Archiving the chat takes the authority with it, pinned commit and all.
    f.store.archive_chat(chat, true).unwrap();
    assert!(f.store.live_chat_push_grant(chat).unwrap().is_none());
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work"), None);
    assert_eq!(
        f.store.expire_stale_chat_push_grants(None).unwrap(),
        0,
        "archive revokes the grant in its own transaction"
    );
    assert_eq!(
        f.store
            .chat_push_grant_for_request(chat, "ask-1")
            .unwrap()
            .unwrap()
            .state,
        "expired"
    );
    f.store.archive_chat(chat, false).unwrap();

    // A grant another process minted is a row, not permission. The identity is immutable,
    // so this is written the only way a previous process could have left it.
    let stale = f.chat(ChatMode::Single);
    rusqlite::Connection::open(&f.db).unwrap().execute(
        "INSERT INTO chat_push_grant(chat_id,request_id,mode,message,workspace_path,workspace_epoch,
            branch,origin_url,head_sha,supervisor_pid,supervisor_identity,commit_sha,state,
            created_at,updated_at)
         VALUES(?1,'before-restart','single',?2,?3,0,'work','/dev/null',?4,999999999,'other',?4,
            'armed','now','now')",
        rusqlite::params![stale, ASKED, f.repo.to_string_lossy(), commit],
    ).unwrap();
    assert!(f.store.live_chat_push_grant(stale).unwrap().is_none());
    assert_eq!(f.store.expire_stale_chat_push_grants(None).unwrap(), 1);
    assert_eq!(f.remote_sha("work"), None);

    // A turn that ends without asking for publication publishes nothing either.
    let (_, node) = f.send(chat, ASKED, "ask-2").await;
    let node = node.unwrap();
    f.turn_ends(chat, node);
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work"), None);
    assert!(f.notes(chat).contains(&"Push not performed".to_string()));
}

#[tokio::test]
async fn pinned_permission_cannot_survive_stop_new_work_or_a_branch_switch() {
    let _env = ENV.lock().await;
    for action in ["stop", "new_turn", "branch"] {
        let mut f = Fixture::new();
        git(&f.repo, &["checkout", "-qb", "work"]);
        let chat = f.chat(ChatMode::Single);
        let (_, node) = f.send(chat, ASKED, "pin").await;
        let node = node.unwrap();
        let commit = f.agent_commits("feature.txt", "done\n", "work");
        chat_push::request_publication(&f.db, chat, node, &commit)
            .await
            .unwrap();
        if action == "stop" {
            f.store.request_chat_stop(chat, node).unwrap();
        }
        f.turn_ends(chat, node);
        if action == "new_turn" {
            let next = f
                .store
                .begin_chat_turn(chat, "different task", "next", &ModelRegistry::local_only())
                .unwrap();
            f.turn_ends(chat, next.node_id);
        }
        if action == "branch" {
            git(&f.repo, &["checkout", "-qb", "another-task"]);
        }
        chat_push::settle(&f.db, chat, node, false).await.unwrap();
        assert_eq!(f.remote_sha("work"), None, "{action}");
        assert_eq!(
            f.store
                .chat_push_grant_for_request(chat, "pin")
                .unwrap()
                .unwrap()
                .state,
            "expired",
            "{action}"
        );
    }
}

#[tokio::test]
async fn a_failed_authority_snapshot_rolls_back_the_turn_instead_of_stranding_it() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);
    let Authority::Solo(mut target) = chat_push::prepare(&mut f.store, chat, ASKED).await.unwrap()
    else {
        panic!("expected exact solo target")
    };
    target.workspace_epoch = 42;
    assert!(f
        .store
        .begin_chat_turn_with_push(
            chat,
            ASKED,
            "atomic",
            &ModelRegistry::local_only(),
            0,
            target
        )
        .is_err());
    assert!(f.store.chat_turns(chat).unwrap().is_empty());
    assert!(f.store.chat(chat).unwrap().active_node_id.is_none());
    assert!(f
        .store
        .chat_push_grant_for_request(chat, "atomic")
        .unwrap()
        .is_none());
}

/// What was approved is a commit on a branch. Moving the branch underneath it is not it.
#[tokio::test]
async fn a_branch_reset_after_the_approval_is_not_what_was_approved() {
    let _env = ENV.lock().await;
    let mut f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "work"]);
    let chat = f.chat(ChatMode::Single);
    let (_, node) = f.send(chat, ASKED, "ask-1").await;
    let node = node.unwrap();
    let commit = f.agent_commits("feature.txt", "done\n", "the asked-for work");
    chat_push::request_publication(&f.db, chat, node, &commit)
        .await
        .unwrap();
    git(&f.repo, &["reset", "-q", "--hard", "HEAD~1"]);
    f.turn_ends(chat, node);
    chat_push::settle(&f.db, chat, node, false).await.unwrap();
    assert_eq!(f.remote_sha("work"), None);
    assert!(f.notes(chat).contains(&"Push refused".to_string()));
}
