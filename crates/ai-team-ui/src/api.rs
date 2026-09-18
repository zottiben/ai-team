//! What the window reads, and how it learns something changed.
//!
//! Every route is a *view* over the store (`ai-team-core`), which is the only thing that
//! writes SQL. Nothing here keeps state of its own: a second window, the CLI, and the
//! desktop shell all read the same rows, and a cache here would be the copy that drifts.
//!
//! Live updates are polled rather than pushed, and that is not laziness. `ait run` is a
//! *different process* writing to the same SQLite file, so there is no in-process channel
//! to subscribe to; the only honest way to notice its work is to look. The poll is keyed
//! on the largest event id, so a quiet run costs one cheap query a tick.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt as _;

use ai_team_core::{Event, NodeRun, Project, Run, Usage};

use crate::error::Result;
use crate::state::AppState;

/// How often to look for new rows. Fast enough that a turn feels live, slow enough that
/// an idle window is not a busy loop on somebody's laptop battery.
const POLL: Duration = Duration::from_millis(400);

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(crate::health::health))
        .route("/projects", get(projects))
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/events", get(run_events))
        .route("/events", get(stream))
}

#[derive(Debug, Serialize)]
struct ProjectView {
    #[serde(flatten)]
    project: Project,
    open_runs: usize,
}

async fn projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectView>>> {
    let store = state.store()?;
    let store = store.lock();
    let mut out = Vec::new();
    for project in store.projects()? {
        let open_runs = store
            .runs(Some(project.id), 100)?
            .into_iter()
            .filter(|run| {
                !matches!(
                    run.status,
                    ai_team_core::RunStatus::Done
                        | ai_team_core::RunStatus::Failed
                        | ai_team_core::RunStatus::Cancelled
                )
            })
            .count();
        out.push(ProjectView { project, open_runs });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct RunQuery {
    project: Option<i64>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

async fn runs(
    State(state): State<AppState>,
    Query(query): Query<RunQuery>,
) -> Result<Json<Vec<Run>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.runs(query.project, query.limit)?))
}

/// One run, with the nodes and the spend that make it readable at a glance.
#[derive(Debug, Serialize)]
struct RunDetail {
    #[serde(flatten)]
    run: Run,
    nodes: Vec<NodeRun>,
    usage: Usage,
}

async fn run(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<RunDetail>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(RunDetail {
        run: store.run(id)?,
        nodes: store.node_runs(id)?,
        usage: store.run_usage(id)?,
    }))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    /// Events after this id. The window keeps the last one it saw and asks for the rest,
    /// so a reconnect never replays what is already on screen.
    after: Option<i64>,
    #[serde(default = "default_event_limit")]
    limit: i64,
}

fn default_event_limit() -> i64 {
    500
}

async fn run_events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<Event>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.events(id, query.after, query.limit)?))
}

/// What changed, as far as a window needs to care.
///
/// Deliberately thin: a tick says *something* happened and hands over the cursor, and the
/// window re-reads whichever view it is showing. Streaming whole rows here would mean
/// this endpoint knowing what every surface renders.
#[derive(Debug, Serialize)]
struct Tick {
    latest_event: i64,
    open_runs: usize,
}

async fn stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(POLL))
        .map(move |_| {
            let tick = state.tick().unwrap_or(Tick {
                latest_event: -1,
                open_runs: 0,
            });
            Ok(SseEvent::default().json_data(tick).unwrap_or_default())
        })
        // Only send when something moved. A window left open overnight should not
        // accumulate a night's worth of identical frames.
        .filter(dedupe());

    // The keep-alive is what stops an idle connection being reaped by anything between
    // here and the browser, even though both ends are on loopback.
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Suppress ticks that say the same thing as the one before.
fn dedupe() -> impl FnMut(&std::result::Result<SseEvent, Infallible>) -> bool {
    let mut last: Option<String> = None;
    move |event| {
        let rendered = format!("{event:?}");
        if last.as_deref() == Some(rendered.as_str()) {
            return false;
        }
        last = Some(rendered);
        true
    }
}

impl AppState {
    /// The cheap "has anything moved?" query behind the stream.
    fn tick(&self) -> Result<Tick> {
        let store = self.store()?;
        let store = store.lock();
        let runs = store.runs(None, 200)?;
        let open_runs = runs
            .iter()
            .filter(|run| {
                !matches!(
                    run.status,
                    ai_team_core::RunStatus::Done
                        | ai_team_core::RunStatus::Failed
                        | ai_team_core::RunStatus::Cancelled
                )
            })
            .count();
        // The largest event id across every run is one indexed lookup, and it moves
        // whenever any node in any run does anything at all.
        let latest_event = store.latest_event_id()?;
        Ok(Tick {
            latest_event,
            open_runs,
        })
    }
}
