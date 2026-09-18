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
use axum::Json as JsonBody;
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
        .route("/runs", axum::routing::post(start))
        .route("/runs/{id}/approvals", get(approvals))
        .route("/runs/{id}/approvals", axum::routing::post(answer))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    project: String,
    prompt: Option<String>,
    plan: Option<String>,
    #[serde(default)]
    replan: bool,
    #[serde(default)]
    plan_only: bool,
}

/// Start a run from the window.
///
/// Returns as soon as the work is under way rather than when it finishes: a run takes
/// minutes, and an HTTP request that waits for one is a request that times out. The run
/// records itself in the database as it goes, which is what the window is already
/// watching - so "started" is genuinely all the caller needs.
async fn start(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<StartRequest>,
) -> Result<Json<serde_json::Value>> {
    // Checked here so an obvious mistake answers immediately rather than failing inside
    // a task nobody is waiting on.
    {
        let store = state.store()?;
        let store = store.lock();
        store.find_project(&request.project)?;
    }

    let spec = ai_team_core::Request {
        project: request.project,
        prompt: request.prompt,
        plan: request.plan,
        width: None,
        replan: request.replan,
        plan_only: request.plan_only,
    };
    tokio::spawn(async move {
        // Its own store: `run_workflow` opens one, and the handler's is held by a mutex
        // the request will drop long before this finishes.
        if let Err(error) = ai_team_core::run_workflow(&spec, |_| {}).await {
            // Nothing to return to - the window reads the run's own rows, and a
            // workflow that failed before creating one has nowhere to write. stderr is
            // where `ait ui` is already being watched from.
            eprintln!("ai-team: run failed: {error}");
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

/// What a run is parked on, if anything.
async fn approvals(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Vec<Event>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.pending_approvals(id)?))
}

#[derive(Debug, Deserialize)]
struct AnswerRequest {
    /// Which node asked. The question belongs to a turn, not to the run as a whole.
    node: i64,
    request: String,
    /// The option the human picked, by the id eve offered it under.
    chose: String,
}

/// Answer a question an agent asked.
///
/// The node records where its eve is listening and the secret it checks, so this reaches
/// the agent over loopback whichever process started it - the window can answer what the
/// terminal's run is parked on.
async fn answer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    JsonBody(request): JsonBody<AnswerRequest>,
) -> Result<Json<serde_json::Value>> {
    let (port, token, session) = {
        let store = state.store()?;
        let store = store.lock();
        let node = store.node_run(request.node)?;
        if node.run_id != id {
            return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
                "that node belongs to a different run",
            )));
        }
        (
            node.eve_port,
            node.eve_token.clone(),
            node.session_id.clone(),
        )
    };

    let (Some(port), Some(token), Some(session)) = (port, token, session) else {
        return Err(crate::error::Error::Core(ai_team_core::Error::invalid(
            "that node has no agent listening - it has already finished or its process \
             has gone",
        )));
    };

    let client = ai_team_core::EveClient::new(u16::try_from(port).unwrap_or(0), &token);
    client
        .respond(&session, &request.request, &request.chose)
        .await?;
    Ok(Json(serde_json::json!({ "answered": true })))
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
