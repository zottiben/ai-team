//! The plan board across projects, and the explicit import of a standalone plan (D27).
//!
//! Four routes, and only the last one writes. Reading a source database is a POST because
//! it carries a filesystem path rather than a resource this server owns - nothing about it
//! is a mutation, and nothing it touches belongs to ai-team. The import is the single
//! approval: it names the source, the plan, the destination chat and the fingerprint of
//! the preview the operator actually read, and the store refuses anything else.
//!
//! These handlers hold the store lock across a file copy and a few SQLite reads. That is
//! deliberate: the work is synchronous and short, and the alternative - handing the path
//! to a blocking task and the result back - would put the destination checks outside the
//! lock that every other planning writer shares.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use ai_team_core::plan_library::{
    PlanImportPreview, PlanImportRequest, PlanImported, PlanLibrary, PlanLibraryFilter,
    PlanSourceSurvey,
};
use ai_team_core::planning::PlanStatus;

use crate::error::{Error, Result};
use crate::state::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/plan-library", get(library))
        .route("/plan-library/source", post(source))
        .route("/plan-library/preview", post(preview))
        .route("/plan-library/import", post(import))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardQuery {
    /// A project slug. Absent is every project, which is the point of the board.
    #[serde(default)]
    project: Option<String>,
    /// Comma separated, because a repeated query parameter is not something every client
    /// spells the same way. Absent is every status.
    #[serde(default)]
    status: Option<String>,
}

async fn library(
    State(state): State<AppState>,
    Query(query): Query<BoardQuery>,
) -> Result<Json<PlanLibrary>> {
    let mut status = Vec::new();
    for name in query
        .status
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        status.push(
            PlanStatus::parse(name)
                .map_err(|error| Error::Core(ai_team_core::Error::invalid(error.to_string())))?,
        );
    }
    let filter = PlanLibraryFilter {
        project: query.project,
        status,
    };
    Ok(Json(state.store()?.lock().plan_library(&filter)?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRequest {
    path: String,
}

/// What a database holds. Nothing is chosen, nothing is written, and the path is read
/// from the request precisely because this must never default to the operator's own
/// planner on startup.
async fn source(
    State(state): State<AppState>,
    Json(request): Json<SourceRequest>,
) -> Result<Json<PlanSourceSurvey>> {
    Ok(Json(state.store()?.lock().read_plan_source(&request.path)?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    path: String,
    plan_id: i64,
}

async fn preview(
    State(state): State<AppState>,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<PlanImportPreview>> {
    Ok(Json(
        state
            .store()?
            .lock()
            .preview_plan_import(&request.path, request.plan_id)?,
    ))
}

async fn import(
    State(state): State<AppState>,
    Json(request): Json<PlanImportRequest>,
) -> Result<Json<PlanImported>> {
    Ok(Json(state.store()?.lock().import_plan(&request)?))
}
