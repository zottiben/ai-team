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

/// Run one turn against `node_run_id`, recording as it goes.
///
/// Returns Pi's session id and what the turn amounted to. The session is stored on the
/// node so a later turn resumes this conversation rather than starting a second one with
/// no history - which is what a repair attempt needs, and what a steering message needs.
pub async fn run<F>(
    store: &mut Store,
    node_run_id: i64,
    turn: &PiTurn,
    mut on_event: F,
) -> Result<(String, TurnOutcome)>
where
    F: FnMut(&PiEvent) + Send,
{
    // The same `TurnOutcome` an eve turn produced, on purpose: it describes a turn - what
    // it recorded, what it spent, how it ended - and none of that is a fact about which
    // runtime took it. Everything downstream of here stayed unchanged because of it.
    let mut summary = TurnOutcome::default();
    // Resume when the node already has a session. A second session would give the model
    // the prompt with none of the conversation that produced the work being repaired.
    let mut turn = turn.clone();
    let resuming = store.node_run(node_run_id)?.session_id;
    if let Some(existing) = &resuming {
        turn.session_id = Some(existing.clone());
    }

    let mut process = PiProcess::start(&turn)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<PiEvent>();

    // The child is driven here and ingested below. `drive` owns the pipes and the
    // callback cannot await, so the two halves have to be separate tasks.
    let driver = tokio::spawn(async move {
        let outcome = process
            .drive(|event| {
                // A closed receiver means the ingesting side gave up; the turn still has
                // to be read to its end or the child blocks on a full pipe.
                let _ = tx.send(event.clone());
            })
            .await;
        outcome
    });

    // Pi announces its session in the first line of the stream, so the id is not known
    // until events start arriving - unlike eve, where a session was created by a request.
    let mut session = resuming.unwrap_or_default();
    let mut cursor = store.node_run(node_run_id)?.stream_cursor;
    let mut batch: Vec<PiEvent> = Vec::with_capacity(BATCH);

    loop {
        let event = rx.recv().await;
        let closed = event.is_none();
        if let Some(event) = event {
            if session.is_empty() {
                if let Some(id) = event.session_id() {
                    session = id.to_string();
                    store.set_node_session(node_run_id, &session)?;
                }
            }
            on_event(&event);
            batch.push(event);
        }

        if (batch.len() >= BATCH || closed) && !batch.is_empty() {
            // Without a session there is nothing stable to key the rows on, which would
            // make a replay duplicate them. Pi sends the session first, so this only
            // happens on a stream that died before saying anything.
            if session.is_empty() {
                batch.clear();
            } else {
                let ingested = store.ingest_pi_events(node_run_id, &session, cursor, &batch)?;
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

    let outcome = driver
        .await
        .map_err(|e| Error::invalid(format!("the Pi turn panicked: {e}")))??;

    if session.is_empty() {
        if let Some(id) = &outcome.session_id {
            session.clone_from(id);
            store.set_node_session(node_run_id, &session)?;
        }
    }

    // A child that died mid-stream never reached `agent_settled`, so nothing in the
    // batches marked the turn terminal. Recording it as failed here is what stops a
    // truncated turn being read as a clean one.
    if outcome.failed || !outcome.settled {
        summary.terminal = Some(crate::model::TerminalState::Failed);
    }
    Ok((session, summary))
}
