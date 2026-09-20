//! Submitting a review, against an agent that is there and one that is not.
//!
//! The quiet failure this guards is a review delivered into a process that has gone: the
//! human believes their comments landed, the agent never hears them, and the work sits
//! unchanged. So the liveness check is a real request, and anything short of a healthy
//! answer has to fall through to writing a slice instead.
//!
//! A stub eve rather than a live one, deliberately. This is about what ai-team does with
//! the answer it gets, and pinning that to a model would make it slow, flaky, and
//! unrunnable on a machine with no subscription.

use std::sync::{Arc, Mutex};

use ai_team_core::{
    CommentStatus, DiffSide, ModelRegistry, NewComment, NewProject, ReviewStatus, RunTrigger,
    Store, Submitted,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How a stubbed agent behaves when ai-team calls.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Agent {
    /// Healthy, and accepts the follow-up.
    Listening,
    /// Answers, but says it is not ok - a process that is up and not able to work.
    Unhealthy,
    /// Healthy, then refuses the follow-up. The message did not land.
    RefusesTheMessage,
}

/// A minimal `/eve/v1` that records what it was sent.
async fn stub_eve(behaviour: Agent) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            tokio::spawn(async move {
                let Some(request) = read_request(&mut socket).await else {
                    return;
                };

                let response = if request.contains("/eve/v1/health") {
                    match behaviour {
                        Agent::Unhealthy => body(200, "{\"ok\":false}"),
                        _ => body(200, "{\"ok\":true}"),
                    }
                } else {
                    sink.lock().unwrap().push(request.clone());
                    match behaviour {
                        Agent::RefusesTheMessage => body(409, "{\"error\":\"session is closed\"}"),
                        _ => body(200, "{}"),
                    }
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    (port, seen)
}

/// Read a whole HTTP request, headers *and* body.
///
/// One `read` is not a request. On Linux the kernel usually hands over both in a single
/// segment and a single read looks correct; on macOS the body routinely arrives second,
/// so the stub recorded only the headers and the assertion about what was sent failed
/// there and nowhere else. Read until `Content-Length` bytes of body have arrived.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut raw = Vec::new();
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        let read = socket.read(&mut buf).await.ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..read]);

        let text = String::from_utf8_lossy(&raw);
        let Some(head_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let length: usize = text[..head_end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0);
        if raw.len() >= head_end + 4 + length {
            break;
        }
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

fn body(status: u16, json: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
        json.len()
    )
}

/// A store with one project, one run, one node and a review of its work.
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

/// Nothing wrote the slice, so the caller can tell steering from planning.
async fn never_plan(_: String, _: String) -> ai_team_core::Result<String> {
    panic!("should not have written a slice");
}

async fn ignore_amend(_: String, _: String) -> ai_team_core::Result<()> {
    Ok(())
}

#[tokio::test]
async fn an_agent_that_is_still_listening_gets_the_comments() {
    let (mut store, review_id, node_id) = fixture();
    let (port, seen) = stub_eve(Agent::Listening).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "sess-1").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let outcome = ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend)
        .await
        .unwrap();

    assert_eq!(
        outcome,
        Submitted::Steered {
            node_run_id: node_id,
            comments: 2,
            // Nothing to tell: this review's run has no orchestrator node.
            told_orchestrator: false,
        }
    );

    // Both comments, to the right session, anchored where the human put them.
    let sent = seen.lock().unwrap().join("\n");
    assert!(sent.contains("/eve/v1/session/sess-1"), "{sent}");
    assert!(sent.contains("name it subtract"), "{sent}");
    assert!(sent.contains("no test covers this"), "{sent}");
    assert!(sent.contains("src/lib.rs:5"), "{sent}");
}

#[tokio::test]
async fn an_agent_that_has_gone_leaves_the_work_on_the_plan() {
    // The ordinary case: the review is read an hour after the run finished.
    let (store, review_id, _) = fixture();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let outcome = ai_team_core::deliver_review(
        &pending,
        node,
        None,
        |title, scope| async move {
            assert!(title.contains("PR1: add sub"));
            // Whoever picks this up has no worktree, and telling it otherwise is an
            // instruction it cannot follow.
            assert!(!scope.contains("worktree you already have"), "{scope}");
            assert!(scope.contains("ai-team/s1"), "{scope}");
            Ok("RV1".into())
        },
        ignore_amend,
    )
    .await
    .unwrap();

    assert_eq!(
        outcome,
        Submitted::Planned {
            slice_key: "RV1".into(),
            comments: 2
        }
    );
}

#[tokio::test]
async fn a_recorded_port_is_not_enough_on_its_own() {
    // The columns say a process *was* started. An hour later something else may hold
    // that port, so liveness is a question asked over the wire.
    let (mut store, review_id, node_id) = fixture();
    let (port, _) = stub_eve(Agent::Unhealthy).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "sess-1").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let planned = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&planned);
    ai_team_core::deliver_review(
        &pending,
        node,
        None,
        move |_, _| async move {
            *flag.lock().unwrap() = true;
            Ok("RV1".into())
        },
        ignore_amend,
    )
    .await
    .unwrap();

    assert!(
        *planned.lock().unwrap(),
        "an unhealthy agent must not swallow the review"
    );
}

#[tokio::test]
async fn an_agent_that_refuses_the_message_is_an_error_not_a_success() {
    // The worst outcome would be reporting success here: the human would believe their
    // comments landed and would never write them again.
    let (mut store, review_id, node_id) = fixture();
    let (port, _) = stub_eve(Agent::RefusesTheMessage).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "sess-1").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let result = ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend).await;

    assert!(result.is_err(), "a refused hand-off must surface");
    // And the review is still open, so it can be submitted again.
    assert!(store.review(review_id).unwrap().submitted_at.is_none());
}

#[tokio::test]
async fn steering_amends_the_slice_so_the_verifier_checks_the_right_spec() {
    // Found by the live demo, not by a fixture. The agent renamed `sub` to `subtract`
    // exactly as the review asked, committed it, and the verifier rejected the commit
    // for not matching a slice that still said `sub`. Feedback that only reaches the
    // agent produces work that is correct and then thrown away.
    let (mut store, review_id, node_id) = fixture();
    let (port, _) = stub_eve(Agent::Listening).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "sess-1").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let amended = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&amended);

    ai_team_core::deliver_review(
        &pending,
        node,
        None,
        never_plan,
        move |key, addition| async move {
            *sink.lock().unwrap() = Some((key, addition));
            Ok(())
        },
    )
    .await
    .unwrap();

    let (key, addition) = amended
        .lock()
        .unwrap()
        .clone()
        .expect("the slice was amended");
    assert_eq!(key, "S1");
    // And it says which text wins, because it is read next to a scope it contradicts.
    assert!(addition.contains("the following wins"), "{addition}");
    assert!(addition.contains("name it subtract"), "{addition}");
}

#[tokio::test]
async fn a_failure_to_amend_the_slice_stops_the_hand_off() {
    // Amending comes first on purpose: a crash between the two steps should leave the
    // plan correct and the agent merely uninformed, never the reverse.
    let (mut store, review_id, node_id) = fixture();
    let (port, seen) = stub_eve(Agent::Listening).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "sess-1").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let result = ai_team_core::deliver_review(&pending, node, None, never_plan, |_, _| async {
        Err(ai_team_core::Error::invalid("ai-planner is not reachable"))
    })
    .await;

    assert!(result.is_err());
    assert!(
        seen.lock().unwrap().is_empty(),
        "the agent must not be told about a change the plan does not record"
    );
}

#[tokio::test]
async fn a_correction_reaches_the_orchestrator_as_well_as_the_author() {
    // The author needs it to fix the code; the orchestrator needs it because it decides what
    // the work *is*, and a correction that only reaches one agent leaves the plan still
    // saying the old thing.
    let (mut store, review_id, node_id) = fixture();
    let (author_port, author_seen) = stub_eve(Agent::Listening).await;
    store.attach_eve(node_id, author_port, "tok").unwrap();
    store.set_node_session(node_id, "author").unwrap();

    // An orchestrator mid-turn on the same run.
    let review = store.review(review_id).unwrap();
    let run_id = review.run_id.unwrap();
    let team = store.project(review.project_id).unwrap().team_id.unwrap();
    let seat = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "orchestrator")
        .unwrap();
    let conductor = store
        .dispatch(run_id, seat.id, None, &ModelRegistry::local_only())
        .unwrap();
    let (orch_port, orch_seen) = stub_eve(Agent::Listening).await;
    store.attach_eve(conductor.id, orch_port, "tok").unwrap();
    store.set_node_session(conductor.id, "conductor").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let orchestrator = ai_team_core::conductor(&store, &pending.review);
    assert!(
        orchestrator.is_some(),
        "the fixture needs an orchestrator node"
    );

    let outcome =
        ai_team_core::deliver_review(&pending, node, orchestrator, never_plan, ignore_amend)
            .await
            .unwrap();

    assert_eq!(
        outcome,
        Submitted::Steered {
            node_run_id: node_id,
            comments: 2,
            told_orchestrator: true,
        }
    );

    // The author is told to fix it.
    let to_author = author_seen.lock().unwrap().join("\n");
    assert!(to_author.contains("/eve/v1/session/author"), "{to_author}");
    assert!(to_author.contains("name it subtract"), "{to_author}");

    // The orchestrator is told the work changed, and explicitly *not* to make the edits -
    // somebody else is already doing that.
    let to_orchestrator = orch_seen.lock().unwrap().join("\n");
    assert!(
        to_orchestrator.contains("/eve/v1/session/conductor"),
        "{to_orchestrator}"
    );
    assert!(
        to_orchestrator.contains("name it subtract"),
        "{to_orchestrator}"
    );
    assert!(
        to_orchestrator.contains("do not need to make these edits"),
        "{to_orchestrator}"
    );
}

#[tokio::test]
async fn an_unreachable_orchestrator_does_not_make_a_delivered_review_look_failed() {
    // It always learns through the amended slice, so failing to reach it costs immediacy
    // rather than the correction.
    let (mut store, review_id, node_id) = fixture();
    let (port, _) = stub_eve(Agent::Listening).await;
    store.attach_eve(node_id, port, "tok").unwrap();
    store.set_node_session(node_id, "author").unwrap();

    let review = store.review(review_id).unwrap();
    let run_id = review.run_id.unwrap();
    let team = store.project(review.project_id).unwrap().team_id.unwrap();
    let seat = store
        .agents(team)
        .unwrap()
        .into_iter()
        .find(|agent| agent.role == "orchestrator")
        .unwrap();
    let conductor = store
        .dispatch(run_id, seat.id, None, &ModelRegistry::local_only())
        .unwrap();
    // A port nothing is listening on.
    store.attach_eve(conductor.id, 1, "tok").unwrap();
    store.set_node_session(conductor.id, "conductor").unwrap();

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let orchestrator = ai_team_core::conductor(&store, &pending.review);

    let outcome =
        ai_team_core::deliver_review(&pending, node, orchestrator, never_plan, ignore_amend)
            .await
            .unwrap();

    assert_eq!(
        outcome,
        Submitted::Steered {
            node_run_id: node_id,
            comments: 2,
            told_orchestrator: false,
        }
    );
}

#[tokio::test]
async fn approving_with_nothing_outstanding_tells_nobody() {
    let (mut store, review_id, _) = fixture();
    for comment in store.comments(review_id).unwrap() {
        store
            .resolve_comment(comment.id, CommentStatus::Resolved)
            .unwrap();
    }

    let pending = ai_team_core::pending_review(&store, review_id).unwrap();
    let node = ai_team_core::responsible(&store, &pending.review);
    let outcome = ai_team_core::deliver_review(&pending, node, None, never_plan, ignore_amend)
        .await
        .unwrap();
    assert_eq!(outcome, Submitted::Accepted);

    store
        .submit_review(review_id, ReviewStatus::Approved)
        .unwrap();
    // And submitting twice is refused rather than sending the comments again.
    assert!(ai_team_core::pending_review(&store, review_id).is_err());
}
