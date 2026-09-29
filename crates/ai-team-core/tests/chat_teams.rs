//! Persisted chat/team identity checks, without launching a model or using operator state.
//! Approval/lease rows below are scope fixtures, not team-runtime or awt acceptance.

use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ai_team_core::planning::{ChatPlan, PlanAccess, PlanAction, PlanActor};
use ai_team_core::{
    Chat, ChatMode, ChatSubmission, ChatTeamPhase, ModelRegistry, NewChat, NewProject, NodeRun,
    NodeStatus, Provider, Reasoning, Store,
};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    chat: Chat,
    registry: ModelRegistry,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(&dir.path().join("team.db")).unwrap();
        let project = store
            .create_project(NewProject {
                name: "Chat team".into(),
                ..Default::default()
            })
            .unwrap();
        store.seed_default_team(project.id).unwrap();
        let chat = store
            .create_chat(NewChat {
                project_id: project.id,
                workspace: dir.path().into(),
                provider: Provider::Local,
                model: "fixture".into(),
                reasoning: Reasoning::High,
            })
            .unwrap();
        Self {
            dir,
            store,
            chat,
            registry: ModelRegistry::local_only(),
        }
    }

    fn team(&mut self) -> ChatSubmission {
        let current = self.store.chat(self.chat.id).unwrap();
        self.store
            .set_chat_mode(current.id, ChatMode::Team, current.rev)
            .unwrap();
        self.store
            .begin_chat_turn(current.id, "plan this", "team-one", &self.registry)
            .unwrap()
    }

    fn agents(&self) -> Vec<ai_team_core::Agent> {
        self.store
            .agents(
                self.store
                    .project(self.chat.project_id)
                    .unwrap()
                    .team_id
                    .unwrap(),
            )
            .unwrap()
    }

    fn planner(&mut self, run_id: i64) -> NodeRun {
        let planner = self
            .agents()
            .into_iter()
            .find(|agent| agent.role == "planner")
            .unwrap();
        let node = self
            .store
            .dispatch(run_id, planner.id, None, &self.registry)
            .unwrap();
        self.store
            .attach_worktree(node.id, &self.chat.workspace_path, None, None)
            .unwrap();
        self.store
            .set_node_status(node.id, NodeStatus::Running)
            .unwrap()
    }

    fn plan(&mut self, actor: PlanActor) -> ChatPlan {
        self.store
            .change_chat_plan(
                self.chat.id,
                actor,
                PlanAction::CreatePlan {
                    expect_revision: 0,
                    title: "One plan".into(),
                    summary: None,
                },
            )
            .unwrap()
    }

    fn conn(&self) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(self.store.path()).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    fn lease(&mut self, run_id: i64) -> (String, ChatPlan) {
        let plan = self.plan(PlanActor::Human);
        let plan = self
            .store
            .change_chat_plan(
                self.chat.id,
                PlanActor::Human,
                PlanAction::AddSlice {
                    expect_revision: plan.revision,
                    key: "S1".into(),
                    title: "First".into(),
                    scope: "Implement it".into(),
                    touches: vec!["src/".into()],
                    demo: "Tests pass".into(),
                },
            )
            .unwrap();
        let slice = &plan.bundle.as_ref().unwrap().slices[0];
        let lease = self.dir.path().join("lease");
        std::fs::create_dir(&lease).unwrap();
        let lease = lease.canonicalize().unwrap().to_string_lossy().into_owned();
        self.conn()
            .execute(
                "UPDATE chat_team_run SET phase = 'building' WHERE run_id = ?1",
                [run_id],
            )
            .unwrap();
        let maker = self
            .agents()
            .into_iter()
            .find(|agent| !agent.read_only)
            .unwrap();
        self.conn().execute(
            "INSERT INTO chat_build_slice (run_id, slice_key, planner_slice_id, approved_rev, worktree_path, lease_state, assigned_agent_id, assigned_agent_rev, agent_snapshot) VALUES (?1, 'S1', ?2, ?3, ?4, 'leased', ?5, ?6, ?7)",
            rusqlite::params![run_id, slice.id, slice.rev, lease, maker.id, maker.rev, serde_json::to_string(&maker).unwrap()],
        ).unwrap();
        (lease, plan)
    }

    fn access(&self, node: i64) -> ai_team_core::Result<PlanAccess> {
        self.store
            .planning_access(self.chat.id, PlanActor::Agent(node))
    }
}

#[test]
fn new_chats_explicitly_default_to_single_agent_mode() {
    let f = Fixture::new();
    assert_eq!(
        serde_json::to_value(f.store.chat(f.chat.id).unwrap()).unwrap()["mode"],
        "single"
    );
}

#[test]
fn changing_mode_keeps_history_and_requires_an_idle_revision() {
    let mut f = Fixture::new();
    let first = f
        .store
        .begin_chat_turn(f.chat.id, "first", "one", &f.registry)
        .unwrap();
    f.store
        .set_node_session(first.node_id, "solo-session")
        .unwrap();
    let active = f.store.chat(f.chat.id).unwrap();
    assert!(f
        .store
        .set_chat_mode(active.id, ChatMode::Team, active.rev)
        .is_err());
    f.store
        .finish_chat_turn(f.chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let plan = f.plan(PlanActor::Human);
    let before = f.store.chat(f.chat.id).unwrap();
    let team = f
        .store
        .set_chat_mode(before.id, ChatMode::Team, before.rev)
        .unwrap();
    assert_eq!(team.id, before.id);
    assert_eq!(team.provider, before.provider);
    assert_eq!(team.model, before.model);
    assert!(f
        .store
        .set_chat_mode(team.id, ChatMode::Single, before.rev)
        .is_err());
    assert_eq!(
        f.store
            .chat_plan(team.id, PlanActor::Human)
            .unwrap()
            .revision,
        plan.revision
    );
    assert_eq!(f.store.chat_turns(team.id).unwrap().len(), 1);
    assert_eq!(
        Store::open(f.store.path())
            .unwrap()
            .chat(team.id)
            .unwrap()
            .mode,
        ChatMode::Team
    );
    f.store
        .set_chat_mode(team.id, ChatMode::Single, team.rev)
        .unwrap();
    let next = f
        .store
        .begin_chat_turn(team.id, "second", "two", &f.registry)
        .unwrap();
    assert_eq!(
        f.store
            .node_run(next.node_id)
            .unwrap()
            .session_id
            .as_deref(),
        Some("solo-session")
    );
    f.store
        .finish_chat_turn(team.id, next.node_id, NodeStatus::Done, None)
        .unwrap();
    let archived = f.store.archive_chat(team.id, true).unwrap();
    assert!(f
        .store
        .set_chat_mode(team.id, ChatMode::Team, archived.rev)
        .is_err());
}

#[test]
fn controller_identity_outlives_the_coordinators_pi_turn() {
    let mut f = Fixture::new();
    let turn = f.team();
    let coordinator = f.store.node_run(turn.node_id).unwrap();
    assert_eq!(coordinator.role, "orchestrator");
    assert!(coordinator.agent_id.is_some());
    let execution = f.store.chat_team_run(turn.run_id).unwrap().unwrap();
    assert_eq!(execution.phase, ChatTeamPhase::Grounding);
    assert!(execution.supervisor_alive());
    assert_eq!(f.access(coordinator.id).unwrap(), PlanAccess::Coordinator);
    assert!(!PlanAccess::Coordinator.tools().contains(&"add_slice"));
    let run = f.store.run(turn.run_id).unwrap();
    assert_eq!(
        run.parallel_width,
        f.store
            .team(run.team_id.unwrap())
            .unwrap()
            .guardrails
            .parallel_width
    );
    let replay = f
        .store
        .begin_chat_turn(f.chat.id, "plan this", "team-one", &f.registry)
        .unwrap();
    assert!(!replay.started);
    assert_eq!(replay.node_id, coordinator.id);
    let planner = f.planner(turn.run_id);
    f.store
        .set_node_status(coordinator.id, NodeStatus::Done)
        .unwrap();
    assert!(f.access(coordinator.id).is_err());
    assert_eq!(f.access(planner.id).unwrap(), PlanAccess::Planner);
    assert!(f
        .store
        .chat_team_run(turn.run_id)
        .unwrap()
        .unwrap()
        .supervisor_alive());
    assert!(f.plan(PlanActor::Agent(planner.id)).bundle.is_some());
    assert_eq!(f.store.chat_team_members(turn.run_id).unwrap().len(), 2);
}

#[test]
fn enrollment_rejects_unapproved_workers_foreign_seats_and_cross_chat_access() {
    let mut f = Fixture::new();
    let turn = f.team();
    let maker = f
        .agents()
        .into_iter()
        .find(|agent| !agent.read_only)
        .unwrap();
    assert!(f
        .store
        .dispatch(turn.run_id, maker.id, Some("not-approved"), &f.registry)
        .is_err());
    assert_eq!(
        f.store.node_runs(turn.run_id).unwrap().len(),
        1,
        "rejected enrollment rolls back its node"
    );
    let planner = f.planner(turn.run_id);
    let other = f
        .store
        .create_chat(NewChat {
            project_id: f.chat.project_id,
            workspace: f.dir.path().into(),
            provider: f.chat.provider,
            model: f.chat.model.clone(),
            reasoning: f.chat.reasoning,
        })
        .unwrap();
    assert!(f
        .store
        .chat_plan(other.id, PlanActor::Agent(planner.id))
        .is_err());
    assert!(f
        .store
        .begin_chat_turn(other.id, "collision", "other", &f.registry)
        .is_err());
    let foreign = f
        .store
        .create_project(NewProject {
            name: "Foreign".into(),
            ..Default::default()
        })
        .unwrap();
    let team = f.store.seed_default_team(foreign.id).unwrap();
    let foreign = f
        .store
        .agents(team.id)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "planner")
        .unwrap();
    assert!(f
        .store
        .dispatch(turn.run_id, foreign.id, None, &f.registry)
        .is_err());
    assert_eq!(f.store.chat_team_members(turn.run_id).unwrap().len(), 2);
}

#[test]
fn solo_lifecycle_cannot_start_finish_or_resume_a_team_and_stop_revokes_members() {
    let mut f = Fixture::new();
    let turn = f.team();
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(ai_team_core::drive_chat(
            f.store.path(),
            f.chat.id,
            turn.node_id,
            false,
        ))
        .unwrap_err();
    assert!(error.to_string().contains("team controller"));
    assert!(f.store.node_run(turn.node_id).unwrap().pi_pid.is_none());
    let planner = f.planner(turn.run_id);
    f.plan(PlanActor::Agent(planner.id));
    f.store.request_chat_stop(f.chat.id, turn.node_id).unwrap();
    assert!(f.access(planner.id).is_err());
    assert!(f
        .store
        .dispatch(turn.run_id, planner.agent_id.unwrap(), None, &f.registry)
        .is_err());
    assert!(f
        .store
        .chat_plan(f.chat.id, PlanActor::Human)
        .unwrap()
        .bundle
        .is_some());
    assert!(f
        .store
        .finish_chat_turn(f.chat.id, turn.node_id, NodeStatus::Done, None)
        .is_err());
    assert!(f.store.claim_chat_resume(f.chat.id, turn.node_id).is_err());
    assert_eq!(
        f.store.chat(f.chat.id).unwrap().active_node_id,
        Some(turn.node_id)
    );
}

#[test]
fn maker_permissions_require_the_exact_approved_lease_and_assigned_slice() {
    let mut f = Fixture::new();
    let turn = f.team();
    let (lease, plan) = f.lease(turn.run_id);
    let agent = f
        .agents()
        .into_iter()
        .find(|agent| !agent.read_only)
        .unwrap();
    let maker = f
        .store
        .dispatch(turn.run_id, agent.id, Some("S1"), &f.registry)
        .unwrap();
    f.store
        .set_node_status(maker.id, NodeStatus::Running)
        .unwrap();
    assert!(
        f.access(maker.id).is_err(),
        "enrollment alone is not a lease"
    );
    assert!(f
        .store
        .attach_worktree(maker.id, &f.chat.workspace_path, None, None)
        .is_err());
    f.store
        .attach_worktree(maker.id, &lease, None, None)
        .unwrap();
    assert_eq!(f.access(maker.id).unwrap(), PlanAccess::Maker);
    #[cfg(unix)]
    assert_lease_alias(&f, turn.run_id, maker.id, &lease);
    let written = f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Agent(maker.id),
            PlanAction::AppendLog {
                expect_revision: plan.revision,
                body: "Working".into(),
                slice: Some("S1".into()),
            },
        )
        .unwrap();
    assert!(f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Agent(maker.id),
            PlanAction::AppendLog {
                expect_revision: written.revision,
                body: "Wrong slice".into(),
                slice: Some("S2".into()),
            }
        )
        .is_err());
    f.conn()
        .execute(
            "UPDATE chat_build_slice SET lease_state = 'released' WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    assert!(f.access(maker.id).is_err());
}

#[cfg(unix)]
fn assert_lease_alias(f: &Fixture, run_id: i64, node_id: i64, lease: &str) {
    let alias = f.dir.path().join("lease-alias");
    std::os::unix::fs::symlink(lease, &alias).unwrap();
    f.conn()
        .execute(
            "UPDATE chat_build_slice SET worktree_path = ?2 WHERE run_id = ?1",
            rusqlite::params![run_id, alias.to_string_lossy()],
        )
        .unwrap();
    assert_eq!(
        f.access(node_id).unwrap(),
        PlanAccess::Maker,
        "an alias is not another worktree"
    );
    f.conn()
        .execute(
            "UPDATE chat_build_slice SET worktree_path = ?2 WHERE run_id = ?1",
            rusqlite::params![run_id, f.chat.workspace_path],
        )
        .unwrap();
    assert!(
        f.access(node_id).is_err(),
        "a genuinely different checkout is not the assigned lease"
    );
    f.conn()
        .execute(
            "UPDATE chat_build_slice SET worktree_path = ?2 WHERE run_id = ?1",
            rusqlite::params![run_id, lease],
        )
        .unwrap();
}

#[test]
fn a_team_edit_cannot_escalate_a_running_readers_snapshot() {
    let mut f = Fixture::new();
    let turn = f.team();
    let (lease, plan) = f.lease(turn.run_id);
    let agent = f
        .agents()
        .into_iter()
        .find(|agent| !agent.read_only)
        .unwrap();
    f.conn()
        .execute(
            "UPDATE agent SET read_only = 1, rev = rev + 1 WHERE id = ?1",
            [agent.id],
        )
        .unwrap();
    let reader = f
        .store
        .dispatch(turn.run_id, agent.id, Some("S1"), &f.registry)
        .unwrap();
    f.store
        .attach_worktree(reader.id, &lease, None, None)
        .unwrap();
    f.store
        .set_node_status(reader.id, NodeStatus::Running)
        .unwrap();
    f.conn()
        .execute(
            "UPDATE agent SET read_only = 0, rev = rev + 1 WHERE id = ?1",
            [agent.id],
        )
        .unwrap();
    assert_eq!(f.access(reader.id).unwrap(), PlanAccess::Reader);
    assert!(f
        .conn()
        .execute(
            "UPDATE chat_team_node SET plan_access = 'planner' WHERE node_id = ?1",
            [reader.id]
        )
        .is_err());
    assert!(f
        .store
        .change_chat_plan(
            f.chat.id,
            PlanActor::Agent(reader.id),
            PlanAction::AppendLog {
                expect_revision: plan.revision,
                body: "Not permitted".into(),
                slice: Some("S1".into()),
            }
        )
        .is_err());
    f.conn()
        .execute(
            "UPDATE chat_build_slice SET lease_state = 'released' WHERE run_id = ?1",
            [turn.run_id],
        )
        .unwrap();
    assert!(f.access(reader.id).is_err());
}

#[test]
fn planning_contention_preserves_writes_and_event_ingest_with_a_large_bundle() {
    let mut f = Fixture::new();
    let turn = f.team();
    let node = f.planner(turn.run_id);
    let plan = f.plan(PlanActor::Human);
    let original_logs = plan.bundle.as_ref().unwrap().log.len();
    // Seed only this temporary, explicitly owned sidecar. Never discover a default DB.
    let mut engine = ai_planner_core::Store::open(&f.store.planning_path().unwrap()).unwrap();
    for index in 0..500 {
        engine
            .append_log(ai_planner_core::NewLog {
                plan_id: plan.bundle.as_ref().unwrap().plan.id,
                body: format!("history-{index}: {}", "x".repeat(1024)),
                ..Default::default()
            })
            .unwrap();
    }
    drop(engine);
    let cursor = f
        .store
        .events(turn.run_id, None, 500)
        .unwrap()
        .last()
        .map(|event| event.id);
    let barrier = Arc::new(Barrier::new(4));
    let started = Instant::now();
    let chat_id = f.chat.id;
    let writers: Vec<_> = (0..2)
        .map(|writer| {
            let path = f.store.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || stress_planning(path, chat_id, node.id, barrier, writer))
        })
        .collect();
    let path = f.store.path().to_path_buf();
    let event_barrier = barrier.clone();
    let ingest =
        std::thread::spawn(move || stress_events(path, turn.run_id, node.id, event_barrier));
    barrier.wait();
    let mut slowest_read = Duration::ZERO;
    let mut bytes = 0;
    for _ in 0..20 {
        let at = Instant::now();
        let snapshot = f.store.chat_plan(chat_id, PlanActor::Human).unwrap();
        bytes = serde_json::to_vec(&snapshot).unwrap().len();
        slowest_read = slowest_read.max(at.elapsed());
    }
    let retries: usize = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .sum();
    let slowest_ingest = ingest.join().unwrap();
    assert_eq!(
        f.store
            .chat_plan(chat_id, PlanActor::Human)
            .unwrap()
            .bundle
            .unwrap()
            .log
            .len(),
        original_logs + 540
    );
    assert_eq!(f.store.events(turn.run_id, cursor, 500).unwrap().len(), 100);
    assert!(bytes > 500 * 1024);
    eprintln!("planning load: 500 x 1KiB historical notes, 40 CAS writes, 100 event appends, 20 full snapshots; elapsed={:?}, slowest event append={slowest_ingest:?}, slowest snapshot+JSON={slowest_read:?}, last bundle={bytes} bytes, stale retries={retries}", started.elapsed());
}

fn stress_planning(
    path: PathBuf,
    chat_id: i64,
    node_id: i64,
    barrier: Arc<Barrier>,
    writer: usize,
) -> usize {
    let mut store = Store::open(&path).unwrap();
    barrier.wait();
    let mut retries = 0;
    for index in 0..20 {
        loop {
            let snapshot = store.chat_plan(chat_id, PlanActor::Agent(node_id)).unwrap();
            match store.change_chat_plan(
                chat_id,
                PlanActor::Agent(node_id),
                PlanAction::AppendLog {
                    expect_revision: snapshot.revision,
                    body: format!("writer-{writer}-{index}"),
                    slice: None,
                },
            ) {
                Ok(_) => break,
                Err(ai_team_core::Error::Invalid(reason))
                    if reason.contains("changed since you read") =>
                {
                    retries += 1;
                    assert!(retries < 1000, "planning writes must make progress");
                }
                Err(error) => panic!("unexpected planning write failure: {error}"),
            }
        }
    }
    retries
}

fn stress_events(path: PathBuf, run_id: i64, node_id: i64, barrier: Arc<Barrier>) -> Duration {
    let mut store = Store::open(&path).unwrap();
    barrier.wait();
    let mut slowest = Duration::ZERO;
    for index in 0..100 {
        let at = Instant::now();
        store
            .append_event(
                run_id,
                ai_team_core::NewEvent::new(
                    ai_team_core::EventKind::Step,
                    format!("load-event-{index}"),
                )
                .on_node(node_id),
            )
            .unwrap();
        slowest = slowest.max(at.elapsed());
        // Keep event writes overlapping the planner's read/modify/write cycles.
        std::thread::sleep(Duration::from_millis(1));
    }
    slowest
}
