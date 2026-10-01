use crate::{
    recover_abandoned_chat_team, recover_abandoned_chat_teams, ChatMode,
    ChatRecoveryState as State, ChatSubmission, ChatTeamPhase, ModelRegistry, NewChat, NewProject,
    Provider, Reasoning, Store,
};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(&dir.path().join("team.db")).unwrap();
        Self { dir, store }
    }
    fn turn(&mut self, name: &str) -> (i64, ChatSubmission) {
        let path = self.dir.path().join(name);
        std::fs::create_dir(&path).unwrap();
        let project = self
            .store
            .create_project(NewProject {
                name: name.into(),
                ..Default::default()
            })
            .unwrap();
        self.store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let chat = self
            .store
            .create_chat_in_mode(
                NewChat {
                    project_id: project.id,
                    workspace: path,
                    provider: Provider::Local,
                    model: "fixture".into(),
                    reasoning: Reasoning::High,
                },
                ChatMode::Team,
            )
            .unwrap();
        let turn = self
            .store
            .begin_chat_turn(chat.id, name, name, &ModelRegistry::local_only())
            .unwrap();
        (chat.id, turn)
    }
    fn state(&self, chat: i64) -> State {
        self.store.chat_team_recovery_scan(Some(chat)).unwrap()[0].state
    }
}

#[test]
fn legacy_plan_readers_do_not_resolve_a_chat_sidecar_through_aip() {
    let mut f = Fixture::new();
    let (chat, _) = f.turn("seed");
    let project = f.store.chat(chat).unwrap().project_id;
    let workspace = f.dir.path().join("shared");
    std::fs::create_dir(&workspace).unwrap();
    let old = f
        .store
        .create_run_in(
            project,
            "legacy",
            crate::RunTrigger::Manual,
            Some(&workspace),
        )
        .unwrap();
    f.store.set_run_plan(old.id, "legacy-plan").unwrap();
    let chat = f
        .store
        .create_chat(NewChat {
            project_id: project,
            workspace: workspace.clone(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let turn = f
        .store
        .begin_chat_turn(chat.id, "new", "new", &ModelRegistry::local_only())
        .unwrap();
    f.store
        .db()
        .conn()
        .execute(
            "UPDATE run SET plan_slug = 'chat-shadow' WHERE id = ?1",
            [turn.run_id],
        )
        .unwrap();
    assert_eq!(
        f.store
            .plan_in_workspace(project, &workspace)
            .unwrap()
            .as_deref(),
        Some("legacy-plan")
    );
    assert_eq!(f.store.plans_in_use(project).unwrap(), vec!["legacy-plan"]);
    assert_eq!(
        f.store.runs_in_workspace(project, &workspace, 1).unwrap()[0].id,
        old.id
    );
    assert_eq!(f.store.legacy_runs(Some(project), 1).unwrap()[0].id, old.id);
    assert_eq!(f.store.legacy_runs(None, 1).unwrap()[0].id, old.id);
    assert_eq!(
        f.store.runs(None, 10).unwrap().len(),
        3,
        "generic evidence still includes chats"
    );
    f.store.db().conn().execute(
        "UPDATE node_run SET task_key = 'T1', slice_key = 'S1', branch = 'chat-draft', push_replaces = 'remote-commit', pi_pid = 123456 WHERE id = ?1",
        [turn.node_id],
    ).unwrap();
    let node = f.store.node_run(turn.node_id).unwrap();
    assert_eq!(node.task_key.as_deref(), Some("T1"));
    assert_eq!(node.push_replaces.as_deref(), Some("remote-commit"));
    assert_eq!(node.pi_pid, Some(123_456));
    assert!(f.store.last_turn_on("chat-shadow", "S1").unwrap().is_none());
    assert!(f
        .store
        .latest_pr_node_in(&chat.workspace_path)
        .unwrap()
        .is_none());
}

#[test]
fn legacy_reply_queues_cannot_record_an_undeliverable_chat_message() {
    let mut f = Fixture::new();
    let (_, turn) = f.turn("reply");
    let agent = f.store.node_run(turn.node_id).unwrap().agent_id.unwrap();
    let count = || {
        f.store
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM event", [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let before = count();
    assert!(f
        .store
        .queue_conversation(turn.node_id, agent, "not sent")
        .unwrap_err()
        .to_string()
        .contains("chat controls"));
    assert!(f
        .store
        .queue_node_message(turn.node_id, agent, "not sent")
        .unwrap_err()
        .to_string()
        .contains("chat controls"));
    assert_eq!(f.store.waiting_for_node(agent, turn.node_id).unwrap(), 0);
    assert_eq!(
        f.store
            .db()
            .conn()
            .query_row("SELECT COUNT(*) FROM event", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        before
    );
}

#[test]
fn the_legacy_clock_leaves_chat_recovery_and_evidence_alone() {
    let mut f = Fixture::new();
    let (chat, team) = f.turn("clock");
    let project = f.store.chat(chat).unwrap().project_id;
    let workspace = f.dir.path().join("solo");
    std::fs::create_dir(&workspace).unwrap();
    let solo = f
        .store
        .create_chat(NewChat {
            project_id: project,
            workspace,
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    let solo = f
        .store
        .begin_chat_turn(solo.id, "solo", "solo", &ModelRegistry::local_only())
        .unwrap();
    let legacy = f
        .store
        .create_run(project, "legacy", crate::RunTrigger::Manual)
        .unwrap();
    f.store.set_run_supervisor(legacy.id, 999_999).unwrap();
    let before = serde_json::to_value(f.store.chat_team_run(team.run_id).unwrap()).unwrap();
    let nodes = [team.node_id, solo.node_id]
        .map(|id| serde_json::to_value(f.store.node_run(id).unwrap()).unwrap());
    let settled = f.store.settle_abandoned_runs(|_| false).unwrap();
    assert_eq!(
        settled.iter().map(|entry| entry.run.id).collect::<Vec<_>>(),
        vec![legacy.id]
    );
    assert_eq!(
        serde_json::to_value(f.store.chat_team_run(team.run_id).unwrap()).unwrap(),
        before
    );
    assert_eq!(
        [team.node_id, solo.node_id]
            .map(|id| serde_json::to_value(f.store.node_run(id).unwrap()).unwrap()),
        nodes
    );
}

#[tokio::test]
async fn synchronous_metadata_keeps_its_controller_journal_inside_tokio() {
    let mut f = Fixture::new();
    let (chat, turn) = f.turn("metadata");
    let (_, owner) = f
        .store
        .claim_chat_team_planning(chat, turn.node_id)
        .unwrap();
    let output = owner
        .track(async {
            let mut command = std::process::Command::new("/bin/echo");
            command.arg("metadata");
            crate::command::run_blocking(command, std::time::Duration::from_secs(3), 1024)
        })
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"metadata\n");
    let children = f.store.chat_children(turn.run_id).unwrap();
    assert_eq!(
        children.len(),
        1,
        "the synchronous bridge lost the task-local receipt"
    );
    assert_eq!(children[0].state, "drained");
    assert!(children[0].pid.is_some());
    assert_eq!(f.state(chat), State::Active);
}

#[tokio::test]
async fn discovery_distinguishes_reservations_receipts_and_abandoned_tasks() {
    let mut f = Fixture::new();
    let (chat, turn) = f.turn("one");
    let original = f.store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert_eq!(f.state(chat), State::PendingDispatch);
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        State::PendingDispatch
    );
    assert_eq!(
        f.store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        original.rev
    );
    let stale = f
        .store
        .chat_team_recovery_scan(Some(chat))
        .unwrap()
        .remove(0)
        .target;
    let (_, owner) = f
        .store
        .claim_chat_team_planning(chat, turn.node_id)
        .unwrap();
    let sibling = owner.clone();
    assert_eq!(f.state(chat), State::Active);
    drop(owner);
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        State::Active
    );
    assert!(crate::recover_chat_team_processes(f.store.path(), &stale)
        .await
        .is_err());
    drop(sibling);
    assert_eq!(
        f.state(chat),
        State::Recoverable,
        "a live host PID is not a live task"
    );
    assert!(
        crate::recover_chat_team_processes(f.store.path(), &stale)
            .await
            .is_err(),
        "a free lock does not make stale evidence current"
    );
    let recovered = recover_abandoned_chat_team(f.store.path(), chat)
        .await
        .unwrap();
    assert_eq!(recovered[0].state, State::Recovered);
    let execution = f.store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert!(execution.quiescent);
    assert_eq!(execution.phase, ChatTeamPhase::Blocked);
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        State::Quiescent
    );
    assert_eq!(
        f.store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        execution.rev
    );
    assert_eq!(f.store.node_runs(turn.run_id).unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_startups_drain_once_and_admission_stays_chat_scoped() {
    let mut f = Fixture::new();
    let (chat, turn) = f.turn("first");
    let (other, other_turn) = f.turn("second");
    for (id, node) in [(chat, turn.node_id), (other, other_turn.node_id)] {
        let (_, owner) = f.store.claim_chat_team_planning(id, node).unwrap();
        drop(owner);
    }
    let db = f.store.path().to_owned();
    let one = tokio::spawn(async move { recover_abandoned_chat_team(&db, chat).await.unwrap() });
    let db = f.store.path().to_owned();
    let two = tokio::spawn(async move { recover_abandoned_chat_team(&db, chat).await.unwrap() });
    let entries = one
        .await
        .unwrap()
        .into_iter()
        .chain(two.await.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.state == State::Recovered)
            .count(),
        1
    );
    assert_eq!(f.store.chat_child_epoch(turn.run_id).unwrap(), 2);
    assert_eq!(f.store.chat_child_epoch(other_turn.run_id).unwrap(), 1);
    assert_eq!(f.state(other), State::Recoverable);
    assert!(recover_abandoned_chat_team(f.store.path(), i64::MAX)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path())
            .await
            .unwrap()
            .iter()
            .filter(|entry| entry.state == State::Recovered)
            .count(),
        1
    );
}

#[tokio::test]
async fn uncertainty_parks_without_retrying_or_blocking_other_chats() {
    let mut f = Fixture::new();
    let (chat, turn) = f.turn("unknown-spawn");
    let (_, owner) = f
        .store
        .claim_chat_team_planning(chat, turn.node_id)
        .unwrap();
    f.store
        .begin_chat_child(
            &owner,
            "pi",
            "fixture",
            None,
            &crate::chat::team::children::boot().unwrap(),
        )
        .unwrap();
    drop(owner);
    let (other, other_turn) = f.turn("recoverable");
    let (_, owner) = f
        .store
        .claim_chat_team_planning(other, other_turn.node_id)
        .unwrap();
    drop(owner);
    let entries = recover_abandoned_chat_teams(f.store.path()).await.unwrap();
    assert_eq!(entries[0].state, State::NeedsInspection);
    assert!(entries[0]
        .reason
        .as_ref()
        .unwrap()
        .contains("spawn intent has no process identity"));
    assert_eq!(entries[1].state, State::Recovered);
    let execution = f.store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert!(!execution.quiescent);
    assert_eq!(f.store.chat_child_epoch(turn.run_id).unwrap(), 2);
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        State::NeedsInspection
    );
    assert_eq!(
        f.store.chat_team_run(turn.run_id).unwrap().unwrap().rev,
        execution.rev
    );
    assert_eq!(f.store.chat_child_epoch(turn.run_id).unwrap(), 2);
}

#[tokio::test]
async fn dead_admitters_are_recoverable_but_old_protocols_and_bad_locks_are_not() {
    let mut f = Fixture::new();
    let (chat, turn) = f.turn("dead-admission");
    // A real exited process, not a guessed unused PID. Reuse of that PID cannot
    // match the original admitting host's saved start identity.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = i64::from(child.id());
    child.wait().unwrap();
    f.store
        .db()
        .conn()
        .execute(
            "UPDATE chat_team_run SET supervisor_pid = ?1 WHERE run_id = ?2",
            [pid, turn.run_id],
        )
        .unwrap();
    assert_eq!(
        recover_abandoned_chat_teams(f.store.path()).await.unwrap()[0].state,
        State::Recovered
    );
    assert!(f
        .store
        .claim_chat_team_planning(chat, turn.node_id)
        .is_err());
    let (old, old_turn) = f.turn("old-protocol");
    f.store
        .db()
        .conn()
        .execute(
            "UPDATE chat_team_run SET controller_protocol = 0, child_journal = 0 WHERE run_id = ?1",
            [old_turn.run_id],
        )
        .unwrap();
    assert_eq!(f.state(old), State::NeedsInspection);
    let (broken, broken_turn) = f.turn("lock-error");
    let path = super::ownership::lock_path(f.store.path(), broken_turn.run_id).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    assert_eq!(f.state(broken), State::NeedsInspection);
    assert!(
        f.store
            .claim_chat_team_planning(old, old_turn.node_id)
            .is_err(),
        "an unjournalled legacy reservation cannot be reinterpreted as a fresh admission"
    );
}
