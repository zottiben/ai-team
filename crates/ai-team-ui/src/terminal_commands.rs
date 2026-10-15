//! Command pads, and the commands pressing one runs.
//!
//! A pad is not a macro that types for you. Pressing one starts a pty of its own running
//! exactly the confirmed command, so nothing is appended to a shell the operator is
//! using - not while a build is running in it, and not on top of half a line they have
//! typed - and the exit status that comes back belongs to the command rather than to a
//! shell that happened to end.
//!
//! Four rules this module holds:
//!
//! - **The press carries what was confirmed.** The pad's stored command and revision must
//!   still be the ones the operator read, and the checkout must still be the one the
//!   confirmation named, or nothing runs. A one-press button edited in another window - or
//!   a deck that moved to a different worktree while the dialog was open - is a button
//!   that runs something nobody read.
//! - **A press happens once.** The window mints a request id when it opens the
//!   confirmation, and the same id arriving again - a lost response, a reload, a double
//!   click - is answered with the receipt the first one left. A command is never run twice
//!   because an answer went missing, and an id that comes back carrying different bytes is
//!   refused rather than answered with somebody else's receipt.
//! - **The record comes before the process.** The row is written while nothing is running,
//!   carrying the token the spawn will use, so there is no window in which a command is
//!   running that nothing has recorded. What that costs - a row that can outlive a crash
//!   still saying `intent` - is reported as an uncertain outcome and never re-run.
//! - **An outcome nobody saw is unknown.** A row left running by an app that exited is
//!   reported as unknown, never as a pass and never as still running.

use ai_team_core::{Dispatch, DispatchClaim, PressRequest, TerminalCommandRow, TerminalPad};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{api::worktree_for, error::Result, state::AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/terminals/pads", get(pads).post(add_pad))
        .route("/terminals/pads/{id}", post(edit_pad))
        .route("/terminals/pads/{id}/remove", post(remove_pad))
        .route("/terminals/commands", get(commands).post(run_command))
}

#[derive(Debug, Deserialize)]
struct Project {
    project: String,
}

#[derive(Debug, Deserialize)]
struct PadEdit {
    project: String,
    label: String,
    command: String,
}

/// Where a command runs: the same checkout the terminal's own sessions are opened in.
#[derive(Debug, Deserialize)]
struct Where {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
}

/// A press: everything the operator was shown before pressing, and the id this press is
/// known by for as long as its row exists.
#[derive(Debug, Deserialize)]
struct Press {
    #[serde(flatten)]
    at: Where,
    /// Minted by the window when the confirmation opened. The same on every retry of this
    /// one press, and never reused for another.
    request_id: String,
    pad: i64,
    /// The pad's revision as the confirmation read it.
    revision: i64,
    confirmed: String,
    /// The checkout the confirmation named, resolved. Sent back so the press can be
    /// refused if this deck has since moved, rather than running in a checkout nobody
    /// confirmed - the dialog's target is a display, and a display goes stale.
    checkout: String,
}

/// What a checkout's command deck looks like right now.
#[derive(Debug, Serialize)]
struct Deck {
    worktree: String,
    /// Commands still running, as this process measures them.
    running: Vec<Running>,
    /// What has been run here, newest first, with each row's outcome as it is known.
    history: Vec<Receipt>,
}

#[derive(Debug, Serialize)]
struct Running {
    token: String,
    session: u64,
    label: String,
    command: String,
    duration_ms: u64,
    lines: u64,
    lines_exact: bool,
}

/// A history row, with its state said rather than inferred from empty columns.
#[derive(Debug, Serialize)]
struct Receipt {
    #[serde(flatten)]
    row: TerminalCommandRow,
    state: CommandState,
}

/// What a press answers with: its receipt, and whether this call is the one that ran it.
#[derive(Debug, Serialize)]
struct Pressed {
    #[serde(flatten)]
    receipt: Receipt,
    /// True when this request had already been made and nothing new was started. The
    /// window shows the receipt it was given rather than reporting a second run.
    replayed: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum CommandState {
    /// This process started it and it is still going.
    Running,
    /// It ended and its exit status was read.
    Finished,
    /// It was started by an app session that is gone, so no outcome was ever observed.
    /// Not a pass, and not still running.
    Unknown,
    /// It was recorded, and whether a process ever started was never written down - a
    /// crash between the record and the spawn. It may have run. It is not re-run, and it
    /// is not reported as a pass or as a failure.
    Uncertain,
    /// The spawn failed with nothing left running. Provably nothing ran.
    Refused,
}

/// How a row reads, given whether this process is watching its command.
///
/// The one case worth naming: a row still saying `intent` whose token this deck knows is
/// *running*. The record of its start was lost, not its process - and this process is
/// holding it.
fn state_of(row: &TerminalCommandRow, known: bool) -> CommandState {
    if row.finished_at.is_some() {
        return if row.exit_code.is_some() {
            CommandState::Finished
        } else {
            CommandState::Unknown
        };
    }
    match row.dispatch {
        Dispatch::Refused => CommandState::Refused,
        _ if known => CommandState::Running,
        Dispatch::Started => CommandState::Unknown,
        Dispatch::Intent => CommandState::Uncertain,
    }
}

async fn pads(
    State(state): State<AppState>,
    Query(at): Query<Project>,
) -> Result<Json<Vec<TerminalPad>>> {
    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(&at.project)?;
    Ok(Json(store.terminal_pads(project.id)?))
}

async fn add_pad(
    State(state): State<AppState>,
    Json(pad): Json<PadEdit>,
) -> Result<Json<TerminalPad>> {
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&pad.project)?;
    Ok(Json(store.add_terminal_pad(
        project.id,
        &pad.label,
        &pad.command,
    )?))
}

async fn edit_pad(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(pad): Json<PadEdit>,
) -> Result<Json<TerminalPad>> {
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&pad.project)?;
    Ok(Json(store.edit_terminal_pad(
        project.id,
        id,
        &pad.label,
        &pad.command,
    )?))
}

async fn remove_pad(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(at): Json<Project>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&at.project)?;
    store.remove_terminal_pad(project.id, id)?;
    Ok(Json(serde_json::json!({ "removed": id })))
}

/// Settle whatever has finished, then answer with this checkout's deck.
///
/// Only observed outcomes settle a row. Acknowledge each receipt after its idempotent
/// durable write, so a database failure leaves the original evidence available to retry.
async fn commands(State(state): State<AppState>, Query(at): Query<Where>) -> Result<Json<Deck>> {
    let worktree = worktree_for(&state, &at.project, at.node, at.workspace.as_deref()).await?;
    let (running, settled) = state.terminals().poll_commands(Some(&worktree));
    let store = state.store()?;
    let mut store = store.lock();
    for outcome in &settled {
        store.settle_terminal_command(outcome)?;
        state.terminals().acknowledge_command(&outcome.token);
    }
    let project = store.find_project(&at.project)?;
    let history = store
        .terminal_command_history(project.id, &worktree.to_string_lossy())?
        .into_iter()
        .map(|row| {
            let known = state.terminals().knows_command(&row.token);
            let state = state_of(&row, known);
            Receipt { row, state }
        })
        .collect();
    Ok(Json(Deck {
        worktree: worktree.to_string_lossy().into_owned(),
        running: running
            .into_iter()
            .map(|receipt| Running {
                token: receipt.token,
                session: receipt.session,
                label: receipt.label,
                command: receipt.command,
                duration_ms: receipt.duration_ms,
                lines: receipt.lines,
                lines_exact: receipt.lines_exact,
            })
            .collect(),
        history,
    }))
}

/// Run one pad, in its own pty, in this checkout - or hand back the receipt of the press
/// this request already made.
///
/// The order is the whole of it: resolve the checkout, check it is the one that was
/// confirmed, record the press, and only then start anything. Each step before the record
/// refuses with nothing run and nothing written, and is an error because there is no row
/// to speak for it; from the record on, every outcome - including "it started nothing" -
/// comes back as that row rather than as a guess.
async fn run_command(
    State(state): State<AppState>,
    Json(press): Json<Press>,
) -> Result<Json<Pressed>> {
    let worktree = worktree_for(
        &state,
        &press.at.project,
        press.at.node,
        press.at.workspace.as_deref(),
    )
    .await?;
    // A dialog names a checkout; by the time Run it is pressed the deck may have been
    // pointed somewhere else. Running "the current one" would be running in a checkout
    // nobody read.
    if worktree.to_string_lossy() != press.checkout {
        return Err(ai_team_core::Error::invalid(format!(
            "this deck is now on {}, not the checkout you confirmed; nothing was run",
            worktree.display()
        ))
        .into());
    }

    let claim = {
        let store = state.store()?;
        let mut store = store.lock();
        let project = store.find_project(&press.at.project)?;
        store.claim_terminal_dispatch(
            project.id,
            &PressRequest {
                request_id: &press.request_id,
                worktree: &worktree.to_string_lossy(),
                pad: press.pad,
                revision: press.revision,
                confirmed: press.confirmed.trim(),
            },
        )?
    };
    let claimed = match claim {
        // Already made. The first press's row is the answer, whatever state it is in;
        // starting the command again because an answer went missing is the one failure
        // this whole arrangement exists to prevent.
        DispatchClaim::Replay(row) => {
            let known = state.terminals().knows_command(&row.token);
            let reads = state_of(&row, known);
            return Ok(Json(Pressed {
                receipt: Receipt { row, state: reads },
                replayed: true,
            }));
        }
        DispatchClaim::Fresh(claimed) => claimed,
    };

    // Refused before anything is started, like any other app-owned session: an update
    // that is installing must not have a command appear underneath it.
    let permit = match state.updates().admit_terminal() {
        Ok(permit) => permit,
        Err(problem) => return refused(&state, &claimed.token, &problem.to_string()),
    };

    let started = match state.terminals().start_command(
        &worktree,
        &claimed.label,
        &claimed.command,
        &claimed.token,
    ) {
        Ok(started) => started,
        // Nothing in `open_builder` can fail once a process exists, so an error here ran
        // nothing - which is what makes calling it refused honest rather than hopeful.
        Err(problem) => return refused(&state, &claimed.token, &problem.to_string()),
    };

    // From here a process is running. It is never killed for a bookkeeping failure, and
    // the press is never called refused: its token is in the deck and on its row, so the
    // outcome still lands where it belongs even if the writes below do not. What each
    // failure owes the operator is the one thing they cannot see - that it *is* running,
    // and under which press - so that pressing again is a choice rather than a reflex.
    let running = |what: &str, problem: &dyn std::fmt::Display| {
        ai_team_core::Error::invalid(format!(
            "{} started in terminal session {}, but {what} failed: {problem}. It was not \
             stopped; do not press it again - its outcome remains associated with this press.",
            claimed.label, started.session
        ))
    };
    state
        .terminals()
        .protect_update(started.session, permit)
        .map_err(|problem| running("holding it against an update", &problem))?;
    let store = state.store()?;
    let row = store
        .lock()
        .record_terminal_start(&claimed.token, started.session)
        .map_err(|problem| running("recording it", &problem))?;
    Ok(Json(Pressed {
        receipt: Receipt {
            row,
            state: CommandState::Running,
        },
        replayed: false,
    }))
}

/// Record that a claimed press started nothing, and answer with that.
///
/// A receipt, not an error, and the difference matters: the window learns its press was
/// answered and gives the next one an id of its own, so a condition that has since
/// cleared - an update that finished, a pty that was not available for a moment - is not
/// a press that can never be made again. Repeating *this* request still runs nothing; it
/// is handed this same row back.
///
/// Only reached where the spawn provably left nothing behind. A press whose start is
/// merely unobserved stays `intent` and reads as uncertain.
fn refused(state: &AppState, token: &str, problem: &str) -> Result<Json<Pressed>> {
    let store = state.store()?;
    let row = store.lock().refuse_terminal_dispatch(token, problem)?;
    Ok(Json(Pressed {
        receipt: Receipt {
            row,
            state: CommandState::Refused,
        },
        replayed: false,
    }))
}
