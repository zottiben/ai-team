//! Submitting a review, against a seat that is still working and one that has finished.
//!
//! The quiet failure this guards is a review that goes nowhere: the human believes their
//! comments landed, the seat never hears them, and the work sits unchanged. On eve that
//! meant probing a live HTTP session, and anything short of a healthy answer fell through
//! to writing a slice.
//!
//! On Pi there is no session to probe, so a correction to a working seat is queued for its
//! next turn (D21) - which removes the failure rather than handling it. What is left to
//! decide is the same question as before, asked of rows instead of a socket: does this
//! seat have a next turn, or does this become a slice for whoever picks the branch up?

use std::sync::{Arc, Mutex};

use ai_team_core::{
    CommentStatus, DiffSide, ModelRegistry, NewComment, NewProject, NodeStatus, ReviewStatus,
    RunTrigger, Store, Submitted,
};

/// A review with two open comments, on a node that has been dispatched.
fn fixture() -> (Store, i64, i64) {
    let mut store = Store::memory().unwrap();
    let project = store
        .create_project(NewProject {
            name: "Demo".into(),
            ..Default::default()
        })
        .unwrap();
    let team = store.seed_default_team(project.id).unwrap();
    let run = store
        .create_run(project.id, "add sub", RunTrigger::Manual)
        .unwrap();
    let agent = store.agents(team.id).unwrap()[0].id;
    let node = store
        .dispatch(run.id, agent, Some("S1"), &ModelRegistry::local_only())
        .unwrap();
    let review = store
        .open_review(
            project.id,
            "PR1: add sub",
            Some(run.id),
            Some(node.id),
            Some("ai-team/s1"),
        )
        .unwrap();

    for (line, text) in [(5, "name it subtract"), (6, "no test covers this")] {
        store
            .comment(
                review.id,
                NewComment {
                    parent_id: None,
                    file_path: Some("src/lib.rs".into()),
                    side: Some(DiffSide::New),
                    line_start: Some(line),
                    line_end: Some(line),
                    author: "human".into(),
                    body: text.into(),
                },
            )
            .unwrap();
    }
    (store, review.id, node.id)
}

/// Mark a node as still at work: a session, and a status that has a next turn.
fn working(store: &mut Store, node_id: i64) {
    store.set_node_session(node_id, "sess-1").unwrap();
    store.set_node_status(node_id, NodeStatus::Running).unwrap();
}

/// Nothing wrote the slice, so the caller can tell steering from planning.
async fn never_plan(_: String, _: String) -> ai_team_core::Result<String> {
    panic!("should not have written a slice");
}

async fn ignore_amend(_: String, _: String) -> ai_team_core::Result<()> {
    Ok(())
}

/// What was queued, and for whom.
type Queued = Arc<Mutex<Vec<(i64, String)>>>;

/// Records what would have been queued, and for whom.
fn recorder() -> (
    Queued,
    impl FnMut(i64, i64, &str) -> ai_team_core::Result<()>,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    (seen, move |_node_id: i64, agent_id: i64, message: &str| {
        sink.lock().unwrap().push((agent_id, message.to_string()));
        Ok(())
    })
}

#[tokio::test]
async fn a_seat_that_is_still_working_gets_the_comments() {
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (queued, sink) = recorder();

    let outcome =
        ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend, sink)
            .await
            .unwrap();

    assert!(
        matches!(outcome, Submitted::Steered { comments: 2, .. }),
        "{outcome:?}"
    );
    let queued = queued.lock().unwrap();
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert!(queued[0].1.contains("name it subtract"), "{queued:?}");
    assert!(queued[0].1.contains("no test covers this"), "{queued:?}");
}

#[tokio::test]
async fn a_seat_that_has_finished_leaves_the_work_on_the_plan() {
    // The ordinary case: a review is usually read long after the turn that produced it.
    let (mut store, review_id, node_id) = fixture();
    store.set_node_session(node_id, "sess-1").unwrap();
    store.set_node_status(node_id, NodeStatus::Done).unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (queued, sink) = recorder();

    let outcome = ai_team_core::deliver_review(
        &pending,
        node,
        None,
        |title, scope| async move {
            assert!(title.starts_with("Review feedback:"), "{title}");
            // Re-addressed: the message for a working seat names a worktree whoever picks
            // this slice up will not have.
            assert!(scope.contains("name it subtract"), "{scope}");
            Ok("RV1".to_string())
        },
        ignore_amend,
        sink,
    )
    .await
    .unwrap();

    assert!(
        matches!(outcome, Submitted::Planned { comments: 2, .. }),
        "{outcome:?}"
    );
    assert!(
        queued.lock().unwrap().is_empty(),
        "a finished seat was sent a message"
    );
}

#[tokio::test]
async fn a_session_alone_is_not_enough_on_its_own() {
    // The session id outlives the turn. Treating a recorded session as "still working"
    // would queue a correction for a seat that has no next turn coming.
    let (mut store, review_id, node_id) = fixture();
    store.set_node_session(node_id, "sess-1").unwrap();
    store.set_node_status(node_id, NodeStatus::Failed).unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    assert!(!ai_team_core::steerable(node.clone()));

    let (_, sink) = recorder();
    let outcome = ai_team_core::deliver_review(
        &pending,
        node,
        None,
        |_, _| async { Ok("RV1".to_string()) },
        ignore_amend,
        sink,
    )
    .await
    .unwrap();
    assert!(matches!(outcome, Submitted::Planned { .. }), "{outcome:?}");
}

#[tokio::test]
async fn a_parked_seat_is_steerable_because_that_is_when_it_most_needs_telling() {
    let (mut store, review_id, node_id) = fixture();
    store.set_node_session(node_id, "sess-1").unwrap();
    store.set_node_status(node_id, NodeStatus::Parked).unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    assert!(ai_team_core::steerable(ai_team_core::responsible(
        &store,
        &pending.review
    )));
}

#[tokio::test]
async fn steering_amends_the_slice_so_the_verifier_checks_the_right_spec() {
    // The verifier checks the commit against the slice spec, so feedback that only reaches
    // the agent produces work that is correct and then rejected for not matching a spec
    // nobody updated.
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);

    let amended = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let sink_amend = Arc::clone(&amended);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (_, sink) = recorder();

    ai_team_core::deliver_review(
        &pending,
        node,
        None,
        never_plan,
        move |key, addition| {
            let sink_amend = Arc::clone(&sink_amend);
            async move {
                sink_amend.lock().unwrap().push((key, addition));
                Ok(())
            }
        },
        sink,
    )
    .await
    .unwrap();

    let amended = amended.lock().unwrap();
    assert_eq!(amended.len(), 1, "{amended:?}");
    assert_eq!(amended[0].0, "S1");
    assert!(amended[0].1.contains("name it subtract"), "{amended:?}");
}

#[tokio::test]
async fn a_failure_to_amend_the_slice_stops_the_hand_off() {
    // Amending first is the recoverable order: a crash between the two leaves the plan
    // right and the agent merely uninformed.
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (queued, sink) = recorder();

    let failed = ai_team_core::deliver_review(
        &pending,
        node,
        None,
        never_plan,
        |_, _| async { Err(ai_team_core::Error::invalid("the planner is down")) },
        sink,
    )
    .await;

    assert!(failed.is_err());
    assert!(
        queued.lock().unwrap().is_empty(),
        "the agent was told about a spec the plan does not have"
    );
}

#[tokio::test]
async fn a_correction_reaches_the_orchestrator_as_well_as_the_author() {
    // The orchestrator decides what the work *is*. Feedback that only reaches the author
    // leaves the plan still saying the old thing.
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let run_id = pending.review.run_id.unwrap();
    let team = store.projects().unwrap()[0].team_id.unwrap();
    let orchestrator = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == ai_team_core::ROOT_ROLE)
        .unwrap();
    let conductor = store
        .dispatch(run_id, orchestrator.id, None, &ModelRegistry::local_only())
        .unwrap();
    working(&mut store, conductor.id);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let conducting = ai_team_core::conductor(&store, &pending.review);
    assert!(
        conducting.is_some(),
        "the fixture needs an orchestrator node"
    );
    let (queued, sink) = recorder();

    let outcome =
        ai_team_core::deliver_review(&pending, node, conducting, never_plan, ignore_amend, sink)
            .await
            .unwrap();

    assert!(
        matches!(
            outcome,
            Submitted::Steered {
                told_orchestrator: true,
                ..
            }
        ),
        "{outcome:?}"
    );

    let queued = queued.lock().unwrap();
    assert_eq!(queued.len(), 2, "{queued:?}");
    let to_orchestrator = &queued[1].1;
    assert!(
        to_orchestrator.contains("name it subtract"),
        "{to_orchestrator}"
    );
    // Told the work changed, and explicitly not to make the edits - two agents acting on
    // one correction is worse than one acting late.
    assert!(
        to_orchestrator.contains("do not need to make these edits"),
        "{to_orchestrator}"
    );
}

#[tokio::test]
async fn an_orchestrator_that_has_finished_does_not_make_a_delivered_review_look_failed() {
    // It always learns through the amended slice, so failing to reach it costs immediacy
    // rather than the correction.
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (queued, sink) = recorder();

    let outcome =
        ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend, sink)
            .await
            .unwrap();

    assert!(
        matches!(
            outcome,
            Submitted::Steered {
                told_orchestrator: false,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(queued.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn approving_with_nothing_outstanding_tells_nobody() {
    let (mut store, review_id, node_id) = fixture();
    working(&mut store, node_id);
    for comment in store.comments(review_id).unwrap() {
        store
            .resolve_comment(comment.id, CommentStatus::Resolved)
            .unwrap();
    }

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let (queued, sink) = recorder();

    let outcome =
        ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend, sink)
            .await
            .unwrap();

    assert_eq!(outcome, Submitted::Accepted);
    assert!(queued.lock().unwrap().is_empty());

    store
        .submit_review(review_id, ReviewStatus::Approved)
        .unwrap();
    // And submitting twice is refused rather than sending the comments again.
    assert!(ai_team_core::pending_review(&store, review_id).is_err());
}
