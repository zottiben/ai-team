//! Driving one Pi turn against a node run, and recording it as it happens.
//!
//! The one thing this must not do is collect the whole stream and write it at the end.
//! `ait ui` and `ait run` are separate processes sharing a SQLite file, and the window
//! learns that anything happened by watching `MAX(event.id)` move (M3-S11). A turn that
//! records nothing until it finishes is a crew panel that says "starting" for four
//! minutes and then jumps to done.
//!
//! So the child is driven in its own task, pushing events down a channel, and the caller
//! ingests them in batches as they arrive. That also sidesteps the borrow problem: the
//! callback into `drive` cannot hold `&mut Store`, because ingesting is async and the
//! callback is not.

use std::time::Duration;

use tokio::sync::mpsc;

use super::event::PiEvent;
use super::process::{PiProcess, PiTurn};
use crate::error::{Error, Result};
use crate::store::Store;
use crate::supervise::TurnOutcome;

/// How many events to hold before writing them.
///
/// Small, because the point is that the window sees a turn while it is happening. Each
/// flush is one transaction, and a turn produces tens of events rather than thousands.
const BATCH: usize = 16;
/// A tool can run for minutes without producing another Pi event. Flush its start before
/// then so the activity stream says what is actually happening instead of looking stuck.
const FLUSH_AFTER: Duration = Duration::from_millis(400);

/// Run one turn against `node_run_id`, recording as it goes.
///
/// Returns Pi's session id and what the turn amounted to. The session is stored on the
/// node so a later turn resumes this conversation rather than starting a second one with
/// no history - which is what a repair attempt needs, and what a steering message needs.
pub async fn run<F>(
    store: &mut Store,
    node_run_id: i64,
    turn: &PiTurn,
    on_event: F,
) -> Result<(String, TurnOutcome)>
where
    F: FnMut(&PiEvent) + Send,
{
    run_until(store, node_run_id, turn, on_event, std::future::pending()).await
}

pub(crate) async fn run_until<F, S>(
    store: &mut Store,
    node_run_id: i64,
    turn: &PiTurn,
    mut on_event: F,
    stop: S,
) -> Result<(String, TurnOutcome)>
where
    F: FnMut(&PiEvent) + Send,
    S: std::future::Future<Output = String> + Send + 'static,
{
    // Resume when the node already has a session. A second session would give the model
    // the prompt with none of the conversation that produced the work being repaired.
    let mut turn = turn.clone();
    let node = store.node_run(node_run_id)?;
    let resuming = node
        .session_retired_at
        .is_none()
        .then_some(node.session_id)
        .flatten();
    if let Some(existing) = &resuming {
        turn.session_id = Some(existing.clone());
    } else {
        // A caller may have prepared the turn before the reset landed. Retirement is the
        // authoritative instruction not to resume, so clear even an explicitly copied id.
        turn.session_id = None;
    }

    let mut process = PiProcess::start(&turn)?;
    if let Err(error) = store.attach_pi_process(node_run_id, process.pid()) {
        // The process already exists, even though its identity could not be persisted.
        // Do not let failure reconciliation release the checkout before it is reaped.
        process.terminate().await.map_err(|cleanup| {
            Error::invalid(format!(
                "{error}; terminating unregistered Pi also failed: {cleanup}"
            ))
        })?;
        return Err(error);
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<PiEvent>();

    // The child is driven here and ingested below. `drive` owns the pipes and the
    // callback cannot await, so the two halves have to be separate tasks.
    let (ingest_failed, failed) = tokio::sync::oneshot::channel::<()>();
    let driver = tokio::spawn(async move {
        let reason = tokio::select! {
            outcome = process.drive(|event| { let _ = tx.send(event.clone()); }) => {
                return (outcome, None);
            }
            reason = stop => reason,
            _ = failed => "The event store stopped accepting this turn.".to_string(),
        };
        let stopped = process
            .terminate()
            .await
            .map(|()| super::process::TurnOutcome::default());
        (stopped, Some(reason))
    });

    // The same `TurnOutcome` an eve turn produced, on purpose: it describes a turn - what
    // it recorded, what it spent, how it ended - and none of that is a fact about which
    // runtime took it. Everything downstream of here stayed unchanged because of it.
    let ingested = ingest_stream(store, node_run_id, resuming, &mut rx, &mut on_event).await;
    let (mut session, mut summary) = match ingested {
        Ok(ingested) => ingested,
        Err(error) => {
            let _ = ingest_failed.send(());
            // Do not release a checkout while an unrecorded tool process is still alive.
            let _ = driver.await;
            return Err(error);
        }
    };

    let (outcome, stopped) = driver
        .await
        .map_err(|e| Error::invalid(format!("the Pi turn panicked: {e}")))?;
    let outcome = outcome?;
    if let Some(reason) = stopped {
        summary.terminal = Some(crate::TerminalState::Cancelled);
        summary.provider_message = Some(reason);
        return Ok((session, summary));
    }

    if session.is_empty() {
        if let Some(id) = &outcome.session_id {
            session.clone_from(id);
            store.set_node_session(node_run_id, &session)?;
        }
    }

    // A child that died mid-stream never reached `agent_settled`, so nothing in the
    // batches marked the turn terminal. Recording it as failed here is what stops a
    // truncated turn being read as a clean one.
    summary.provider_message = outcome
        .said
        .filter(|message| !message.trim().is_empty())
        .or_else(|| {
            let stderr = outcome.stderr.trim();
            (!stderr.is_empty()).then(|| stderr.to_string())
        });
    if outcome.failed || !outcome.settled {
        summary.terminal = Some(crate::model::TerminalState::Failed);
    }
    Ok((session, summary))
}

/// Persist one Pi stream while its producer is still alive.
async fn ingest_stream<F>(
    store: &mut Store,
    node_run_id: i64,
    resuming: Option<String>,
    rx: &mut mpsc::UnboundedReceiver<PiEvent>,
    on_event: &mut F,
) -> Result<(String, TurnOutcome)>
where
    F: FnMut(&PiEvent) + Send,
{
    // Pi announces its session in the first line of the stream, so the id is not known
    // until events start arriving - unlike eve, where a session was created by a request.
    let mut session = resuming.unwrap_or_default();
    let mut cursor = store.node_run(node_run_id)?.stream_cursor;
    let mut batch: Vec<PiEvent> = Vec::with_capacity(BATCH);
    let mut summary = TurnOutcome::default();
    let mut live = super::live::LiveText::default();

    loop {
        let (event, flush_due) = if batch.is_empty() {
            (rx.recv().await, false)
        } else {
            tokio::select! {
                event = rx.recv() => (event, false),
                () = tokio::time::sleep(FLUSH_AFTER) => (None, true),
            }
        };
        let closed = event.is_none() && !flush_due;
        if let Some(event) = event {
            if session.is_empty() {
                if let Some(id) = event.session_id() {
                    session = id.to_string();
                    store.set_node_session(node_run_id, &session)?;
                }
            }
            on_event(&event);
            live.accept(&event);
            batch.push(event);
        }

        if (batch.len() >= BATCH || closed || flush_due) && !batch.is_empty() {
            // Without a session there is nothing stable to key the rows on, which would
            // make a replay duplicate them. Pi sends the session first, so this only
            // happens on a stream that died before saying anything.
            if session.is_empty() {
                batch.clear();
            } else {
                let ingested = store.ingest_pi_events(node_run_id, &session, cursor, &batch)?;
                store.update_chat_preview(node_run_id, &live.text())?;
                cursor += i64::try_from(batch.len()).unwrap_or(0);
                summary.recorded += ingested.recorded;
                summary.duplicates += ingested.duplicates;
                summary.usage += ingested.usage;
                summary.steps += ingested.steps;
                if let Some(terminal) = ingested.terminal {
                    summary.terminal = Some(terminal);
                }
                batch.clear();
            }
        }
        if closed {
            break;
        }
    }
    Ok((session, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, RunTrigger};
    use crate::ModelRegistry;

    fn node(store: &mut Store) -> (i64, i64) {
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        let agent = store
            .agents(team.id)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        let node = store
            .dispatch(run.id, agent.id, Some("S1"), &ModelRegistry::local_only())
            .unwrap();
        (run.id, node.id)
    }

    #[tokio::test]
    async fn a_long_running_tool_is_visible_before_it_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("team.db");
        let mut store = Store::init(&path).unwrap();
        let (run_id, node_id) = node(&mut store);
        let before = store.event_count(run_id).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(PiEvent::parse(r#"{"type":"session","id":"s1","cwd":"/w"}"#).unwrap())
            .unwrap();
        tx.send(
            PiEvent::parse(
                r#"{"type":"tool_execution_start","toolName":"bash","args":{"command":"slow tests"}}"#,
            )
            .unwrap(),
        )
        .unwrap();

        let observe = async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            let observer = rusqlite::Connection::open(&path).unwrap();
            let count = observer
                .query_row(
                    "SELECT COUNT(*) FROM event WHERE run_id = ?1",
                    [run_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            drop(tx);
            count
        };
        let mut on_event = |_: &PiEvent| {};
        let (ingested, visible) = tokio::join!(
            ingest_stream(&mut store, node_id, None, &mut rx, &mut on_event),
            observe
        );
        ingested.unwrap();

        assert_eq!(
            visible,
            before + 1,
            "the in-flight tool call stayed buffered"
        );
    }
}
