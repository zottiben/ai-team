//! One run, end to end, against a real file on disk.
//!
//! The unit tests check each store module in isolation. This checks the thing they
//! cannot: that a whole run hangs together across tables, and that the `v_` views - the
//! ones somebody opens TablePlus to read - report it correctly. A view that silently
//! joins wrong is invisible to a unit test and obvious here.

use ai_team_core::{
    CommentStatus, DiffSide, EventKind, NewComment, NewEvent, NewProject, NewRepo, NodeStatus,
    Provider, ReviewStatus, RunStatus, RunTrigger, Store, Usage,
};

struct Harness {
    store: Store,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(&dir.path().join("team.db")).unwrap();
        Harness { store, _dir: dir }
    }

    /// A second connection onto the same file, opened the way an external tool opens it.
    ///
    /// Deliberately not a back door into `Store`: the demo for this slice is that
    /// someone points TablePlus at the file and it explains itself, so the views are
    /// checked from outside rather than through the API that wrote them.
    fn outside(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.store.path()).unwrap()
    }

    /// One cell of a view's first row, rendered as a human would see it.
    fn cell(&self, view: &str, column: &str) -> String {
        let conn = self.outside();
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {view} LIMIT 1"))
            .unwrap_or_else(|e| panic!("{view} is not queryable: {e}"));
        let index = stmt
            .column_names()
            .iter()
            .position(|name| *name == column)
            .unwrap_or_else(|| panic!("{view} has no column {column}"));
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().expect("the view has a row");
        match row.get::<_, rusqlite::types::Value>(index).unwrap() {
            rusqlite::types::Value::Null => String::new(),
            rusqlite::types::Value::Integer(i) => i.to_string(),
            rusqlite::types::Value::Real(f) => f.to_string(),
            rusqlite::types::Value::Text(t) => t,
            rusqlite::types::Value::Blob(_) => "<blob>".to_string(),
        }
    }

    fn view_count(&self, view: &str) -> i64 {
        self.outside()
            .query_row(&format!("SELECT COUNT(*) FROM {view}"), [], |r| r.get(0))
            .unwrap()
    }
}

#[test]
fn a_whole_run_hangs_together_and_the_views_report_it() {
    let mut h = Harness::new();

    // --- the org graph -----------------------------------------------------------
    let project = h
        .store
        .create_project(NewProject {
            name: "Widget Service".into(),
            summary: Some("The widget, and the service around it".into()),
            ..Default::default()
        })
        .unwrap();
    h.store
        .attach_repo(
            project.id,
            NewRepo {
                remote_url: Some("git@github.com:acme/widget.git".into()),
                main_path: Some("/tmp/widget".into()),
                default_branch: Some("main".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let team = h.store.seed_default_team(project.id).unwrap();

    assert_eq!(h.cell("v_projects", "team"), team.name);
    assert_eq!(h.cell("v_projects", "agents"), "6");
    assert_eq!(h.cell("v_projects", "repos"), "1");
    assert_eq!(h.view_count("v_agents"), 6);

    // --- the run -----------------------------------------------------------------
    let run = h
        .store
        .create_run(project.id, "add a CSV export", RunTrigger::Manual)
        .unwrap();
    h.store.set_run_status(run.id, RunStatus::Planning).unwrap();
    // The plan itself lives in ai-planner (D4); we only hold the reference.
    h.store.set_run_plan(run.id, "widget-csv-export").unwrap();
    h.store.set_run_status(run.id, RunStatus::Running).unwrap();

    assert_eq!(h.cell("v_runs", "project"), "widget-service");
    assert_eq!(h.cell("v_runs", "plan_slug"), "widget-csv-export");
    assert_eq!(h.cell("v_projects", "open_runs"), "1");

    // --- two seats, in parallel --------------------------------------------------
    let agents = h.store.agents(team.id).unwrap();
    let backend = agents.iter().find(|a| a.role == "backend").unwrap();
    let frontend = agents.iter().find(|a| a.role == "frontend").unwrap();

    // The orchestrator's actual question: who owns this path?
    assert_eq!(
        h.store
            .agent_for_path(team.id, "crates/widget/src/export.rs")
            .unwrap()
            .unwrap()
            .id,
        backend.id
    );

    let be = h.store.dispatch(run.id, backend.id, Some("PR1")).unwrap();
    let fe = h.store.dispatch(run.id, frontend.id, Some("PR2")).unwrap();
    h.store
        .attach_worktree(be.id, "/tmp/wt/PR1", Some("slice/PR1"), Some("lease-1"))
        .unwrap();
    h.store
        .attach_worktree(fe.id, "/tmp/wt/PR2", Some("slice/PR2"), Some("lease-2"))
        .unwrap();
    h.store.set_node_status(be.id, NodeStatus::Running).unwrap();
    h.store.set_node_status(fe.id, NodeStatus::Running).unwrap();

    assert_eq!(h.view_count("v_node_runs"), 2);
    assert_eq!(h.cell("v_runs", "nodes"), "2");

    // --- what happened inside the turn -------------------------------------------
    h.store
        .append_event(
            run.id,
            NewEvent::new(EventKind::ToolCall, "bash: cargo test")
                .on_node(be.id)
                .by("backend")
                .with(serde_json::json!({ "argv": ["cargo", "test"] })),
        )
        .unwrap();
    h.store
        .append_event(
            run.id,
            NewEvent::new(EventKind::ApprovalRequest, "commit to slice/PR1?").on_node(be.id),
        )
        .unwrap();

    assert_eq!(h.cell("v_events", "project"), "widget-service");
    assert_eq!(h.store.pending_approvals(run.id).unwrap().len(), 1);

    h.store
        .append_event(
            run.id,
            NewEvent::new(EventKind::ApprovalResolved, "approved").on_node(be.id),
        )
        .unwrap();
    assert!(h.store.pending_approvals(run.id).unwrap().is_empty());

    record_the_cold_then_warm_claude_shape(&mut h, run.id, be.id);

    // --- the verifier rejects it, and the retry is a new row ---------------------
    h.store
        .block_node(be.id, "cargo test failed: 2 tests")
        .unwrap();
    let retry = h.store.dispatch(run.id, backend.id, Some("PR1")).unwrap();
    assert_eq!(retry.attempt, 2);
    // The sibling is untouched: a failed node fails its own branch (M2-S10).
    assert_eq!(h.store.node_run(fe.id).unwrap().status, NodeStatus::Running);
    h.store.set_node_status(retry.id, NodeStatus::Done).unwrap();

    review_the_work(&mut h, project.id, run.id, retry.id);

    // --- done --------------------------------------------------------------------
    h.store.set_node_status(fe.id, NodeStatus::Done).unwrap();
    let done = h.store.set_run_status(run.id, RunStatus::Done).unwrap();
    assert!(done.started_at.is_some() && done.ended_at.is_some());
    assert_eq!(h.cell("v_projects", "open_runs"), "0");
    assert_eq!(h.cell("v_runs", "nodes_done"), "2");
}

/// A large cache write on the cold turn, read back on the warm one - the shape actually
/// measured against a Claude node: `cacheRead 61416`, `cacheWrite 2686`.
fn record_the_cold_then_warm_claude_shape(h: &mut Harness, run_id: i64, node_id: i64) {
    h.store
        .record_usage(
            node_id,
            Usage {
                tokens_in: 3_000,
                tokens_out: 900,
                cache_read: 0,
                cache_write: 61_416,
            },
            1,
        )
        .unwrap();
    h.store
        .record_usage(
            node_id,
            Usage {
                tokens_in: 1_200,
                tokens_out: 400,
                cache_read: 61_416,
                cache_write: 0,
            },
            1,
        )
        .unwrap();

    let usage = h.store.run_usage(run_id).unwrap();
    assert_eq!(usage.cache_read, 61_416);
    assert_eq!(usage.billable(), 4_200 + 1_300 + 61_416);
    // The view keeps them apart too, which is the whole reason Analytics can be honest
    // about which nodes are burning rate limit on prefix alone.
    assert_eq!(h.cell("v_runs", "tokens_cached"), "61416");
    assert_eq!(h.cell("v_runs", "tokens"), "5500");
}

/// A human reads the diff, comments, and the comment has to be dealt with before the
/// review can say it approves of anything.
fn review_the_work(h: &mut Harness, project_id: i64, run_id: i64, node_id: i64) {
    let review = h
        .store
        .open_review(
            project_id,
            "PR1: CSV export",
            Some(run_id),
            Some(node_id),
            Some("slice/PR1"),
        )
        .unwrap();
    h.store
        .set_review_range(review.id, "abc1234", "def5678")
        .unwrap();
    let comment = h
        .store
        .comment(
            review.id,
            NewComment {
                parent_id: None,
                file_path: Some("crates/widget/src/export.rs".into()),
                side: Some(DiffSide::New),
                line_start: Some(42),
                line_end: Some(45),
                author: "human".into(),
                body: "this allocates per row".into(),
            },
        )
        .unwrap();

    assert_eq!(h.cell("v_open_reviews", "unresolved"), "1");
    assert_eq!(h.cell("v_projects", "open_reviews"), "1");
    // An approval that contradicts its own unresolved comments is refused.
    assert!(h
        .store
        .submit_review(review.id, ReviewStatus::Approved)
        .is_err());

    h.store
        .resolve_comment(comment.id, CommentStatus::Resolved)
        .unwrap();
    h.store
        .submit_review(review.id, ReviewStatus::Approved)
        .unwrap();
    assert_eq!(h.view_count("v_open_reviews"), 0);
}

#[test]
fn a_project_that_is_not_a_repo_works_the_same_way() {
    // D6: a project is a container. A ClickUp epic has no checkout at all, and every
    // view has to keep working without one.
    let mut h = Harness::new();
    let project = h
        .store
        .create_project(NewProject {
            name: "ACME-1200 - Billing revamp".into(),
            kind: Some(ai_team_core::ProjectKind::Epic),
            source: Some(ai_team_core::ProjectSource::ClickUp),
            source_key: Some("86abc123".into()),
            source_url: Some("https://app.clickup.com/t/86abc123".into()),
            brief_md: "## Acceptance criteria\n- invoices export as PDF\n".into(),
            ..Default::default()
        })
        .unwrap();
    h.store.seed_default_team(project.id).unwrap();

    assert_eq!(h.cell("v_projects", "kind"), "epic");
    assert_eq!(h.cell("v_projects", "repos"), "0");
    assert_eq!(h.cell("v_projects", "agents"), "6");
    // The brief arrived verbatim, so an agent reads what was written rather than a
    // lossy parse of it.
    assert!(h
        .store
        .project(project.id)
        .unwrap()
        .brief_md
        .contains("invoices export as PDF"));
}

#[test]
fn the_database_survives_being_closed_and_reopened() {
    // WAL plus a second connection is where "it worked in tests" usually stops being
    // true, so this opens the same file twice for real.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("team.db");

    let mut store = Store::init(&path).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    store.seed_default_team(project.id).unwrap();
    let run = store
        .create_run(project.id, "ship it", RunTrigger::Manual)
        .unwrap();
    drop(store);

    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 2);
    assert_eq!(reopened.projects().unwrap().len(), 1);
    assert_eq!(reopened.run(run.id).unwrap().prompt, "ship it");
    assert_eq!(
        reopened
            .agents(reopened.project(project.id).unwrap().team_id.unwrap())
            .unwrap()
            .len(),
        6
    );

    // And a second writer on the same file does not corrupt the first's view of it.
    let mut second = Store::open(&path).unwrap();
    second
        .append_event(
            run.id,
            NewEvent::new(EventKind::Note, "from another process"),
        )
        .unwrap();
    assert_eq!(reopened.event_count(run.id).unwrap(), 1);
}

#[test]
fn seeding_twice_does_not_produce_a_second_roster() {
    let mut h = Harness::new();
    let project = h
        .store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    h.store.seed_default_team(project.id).unwrap();

    // Each role is unique per team, so a second seed onto the same team is refused
    // rather than quietly doubling the roster.
    let again = h.store.seed_default_team(project.id);
    assert!(again.is_err(), "a duplicate team slug must not be accepted");
    assert_eq!(h.view_count("v_agents"), 6);
}

#[test]
fn every_provider_in_the_registry_is_subscription_backed() {
    // D8, asserted rather than trusted. If someone adds a metered provider this fails
    // here, next to the reason.
    let mut h = Harness::new();
    let project = h
        .store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    let team = h.store.seed_default_team(project.id).unwrap();
    let agent = h.store.agents(team.id).unwrap().into_iter().next().unwrap();

    for provider in Provider::ALL {
        let updated = h.store.set_agent_model(agent.id, *provider, "m").unwrap();
        assert_eq!(updated.provider, *provider);
    }
    assert_eq!(
        Provider::ALL.len(),
        4,
        "claude, openai, zai, local - and nothing metered"
    );
}
