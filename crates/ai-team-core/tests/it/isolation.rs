//! A failing node fails its own branch, and nothing else (M2-S10).
//!
//! Proven here rather than only in a live run: whether one node's collapse takes its
//! siblings with it is the kind of property that holds every day until the night it
//! matters, and a demo that happened not to fail proves nothing about it.

use ai_team_core::{
    Guardrails, ModelRegistry, NewProject, NodeStatus, OnFailure, RunStatus, RunTrigger, Store,
    Usage,
};

fn seeded() -> (Store, i64, i64) {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Widget".into(),
            ..Default::default()
        })
        .unwrap();
    let team = store
        .seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())
        .unwrap();
    (store, project.id, team.id)
}

fn seat(store: &Store, team: i64, role: &str) -> i64 {
    store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == role)
        .unwrap()
        .id
}

#[test]
fn a_node_that_runs_out_of_repairs_blocks_its_own_branch_only() {
    let (mut store, project, team) = seeded();
    let registry = ModelRegistry::local_only();
    let run = store
        .create_run(project, "ship both", RunTrigger::Manual)
        .unwrap();

    // Two slices, two seats, two worktrees - the shape `ait run` dispatches.
    let failing = store
        .dispatch(
            run.id,
            seat(&store, team, "backend"),
            Some("PR1"),
            &registry,
        )
        .unwrap();
    let healthy = store
        .dispatch(
            run.id,
            seat(&store, team, "frontend"),
            Some("PR2"),
            &registry,
        )
        .unwrap();
    store
        .attach_worktree(failing.id, "/wt/1", None, None)
        .unwrap();
    store
        .attach_worktree(healthy.id, "/wt/2", None, None)
        .unwrap();

    // One exhausts its repairs and is blocked with the reason attached.
    store
        .block_node(
            failing.id,
            "stopped after 2 repair(s): FAIL test `cargo test`",
        )
        .unwrap();
    store
        .set_node_status(failing.id, NodeStatus::Failed)
        .unwrap();

    // The other finishes, in its own worktree, on its own branch.
    store
        .attach_worktree(healthy.id, "/wt/2", Some("ai-team/pr2"), None)
        .unwrap();
    store.set_node_status(healthy.id, NodeStatus::Done).unwrap();

    let failed = store.node_run(failing.id).unwrap();
    assert_eq!(failed.status, NodeStatus::Failed);
    assert!(failed.blocked_reason.unwrap().contains("cargo test"));
    assert_eq!(failed.branch, None, "nothing was landed for it");

    let done = store.node_run(healthy.id).unwrap();
    assert_eq!(done.status, NodeStatus::Done);
    assert_eq!(done.branch.as_deref(), Some("ai-team/pr2"));
    assert_ne!(done.worktree_path, failed.worktree_path);

    // And the run is blocked rather than failed: there is finished work on it, and
    // calling the whole thing a failure would throw away what the sibling did.
    store.set_run_status(run.id, RunStatus::Blocked).unwrap();
    assert_eq!(store.run(run.id).unwrap().status, RunStatus::Blocked);
}

#[test]
fn a_retry_keeps_the_evidence_of_the_attempt_it_replaced() {
    // D2: a retry is a new row, never an edit. Analytics is made of the first attempt,
    // and a repaired node that overwrote it would report a clean history it never had.
    let (mut store, project, team) = seeded();
    let registry = ModelRegistry::local_only();
    let run = store
        .create_run(project, "ship it", RunTrigger::Manual)
        .unwrap();
    let backend = seat(&store, team, "backend");

    let first = store
        .dispatch(run.id, backend, Some("PR1"), &registry)
        .unwrap();
    store
        .record_usage(
            first.id,
            Usage {
                tokens_in: 1_000,
                tokens_out: 200,
                cache_read: 0,
                cache_write: 0,
            },
            1,
        )
        .unwrap();
    store.block_node(first.id, "FAIL format").unwrap();
    store.set_node_status(first.id, NodeStatus::Failed).unwrap();

    let second = store
        .dispatch(run.id, backend, Some("PR1"), &registry)
        .unwrap();
    store.set_node_status(second.id, NodeStatus::Done).unwrap();

    assert_eq!(first.attempt, 1);
    assert_eq!(second.attempt, 2, "the same slice, one attempt later");
    assert_ne!(first.id, second.id);

    // The failed attempt is still there, with what it spent and why it stopped.
    let kept = store.node_run(first.id).unwrap();
    assert_eq!(kept.status, NodeStatus::Failed);
    assert_eq!(kept.usage.billable(), 1_200);
    assert_eq!(kept.blocked_reason.as_deref(), Some("FAIL format"));

    // And the run's usage is both attempts: a repair costs what it costs.
    assert_eq!(store.run_usage(run.id).unwrap().billable(), 1_200);
}

#[test]
fn escalating_parks_the_work_instead_of_dropping_it() {
    // The difference between the two policies that matter: abort_branch gives up on the
    // slice, escalate keeps it and waits for a person.
    let (mut store, project, team) = seeded();
    let mut guardrails = store.team(team).unwrap().guardrails;
    guardrails.on_failure = OnFailure::Escalate;
    store
        .update_team(team, "Widget team", "", guardrails)
        .unwrap();

    let run = store
        .create_run(project, "ship it", RunTrigger::Manual)
        .unwrap();
    // The policy is snapshotted, so the run carries it even if the team changes again.
    assert_eq!(run.on_failure, OnFailure::Escalate);
    assert_eq!(
        ai_team_core::Fallout::of(run.on_failure),
        ai_team_core::Fallout::Escalate
    );

    let node = store
        .dispatch(
            run.id,
            seat(&store, team, "backend"),
            Some("PR1"),
            &ModelRegistry::local_only(),
        )
        .unwrap();
    store
        .block_node(node.id, "the verifier rejected it twice")
        .unwrap();
    store.set_node_status(node.id, NodeStatus::Parked).unwrap();

    let parked = store.node_run(node.id).unwrap();
    assert_eq!(parked.status, NodeStatus::Parked);
    assert!(parked.blocked_reason.is_some(), "parked with its reason");
}

#[test]
fn guardrails_are_a_team_setting_and_every_run_takes_its_own_copy() {
    let (mut store, project, team) = seeded();
    let tight = Guardrails {
        parallel_width: 1,
        budget_tokens_run: Some(1_000),
        budget_tokens_node: Some(500),
        budget_seconds_run: Some(60),
        budget_seconds_node: Some(30),
        max_turns_node: Some(3),
        max_repairs: 0,
        on_failure: OnFailure::AbortBranch,
    };
    store.update_team(team, "Widget team", "", tight).unwrap();

    let run = store
        .create_run(project, "ship it", RunTrigger::Manual)
        .unwrap();
    assert_eq!(run.parallel_width, 1);
    assert_eq!(run.budget_tokens, Some(1_000));
    assert_eq!(run.budget_tokens_node, Some(500));
    assert_eq!(run.budget_seconds, Some(60));
    assert_eq!(run.budget_seconds_node, Some(30));
    assert_eq!(run.max_turns_node, Some(3));
    assert_eq!(run.max_repairs, 0);
    assert_eq!(run.on_failure, OnFailure::AbortBranch);

    // Loosening the team afterwards must not loosen a run already going.
    store
        .update_team(team, "Widget team", "", Guardrails::default())
        .unwrap();
    let unchanged = store.run(run.id).unwrap();
    assert_eq!(unchanged.budget_tokens_node, Some(500));
    assert_eq!(unchanged.max_turns_node, Some(3));
    assert_eq!(unchanged.max_repairs, 0);
    assert_eq!(unchanged.on_failure, OnFailure::AbortBranch);
}
