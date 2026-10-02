//! Authenticated project setup. Browser approval names a saved preview, not choices.

use ai_team_core::toolbox::{self, Preview, Scan, Selection};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::{error::Result, state::AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/toolbox", get(scan))
        .route("/projects/{project}/toolbox/history", get(history))
        .route("/projects/{project}/toolbox/preview", post(preview))
        .route("/projects/{project}/toolbox/previews/{id}", get(saved))
        .route(
            "/projects/{project}/toolbox/previews/{id}/apply",
            post(apply),
        )
}

async fn scan(State(state): State<AppState>, Path(project): Path<i64>) -> Result<Json<Vec<Scan>>> {
    let store = state.store()?;
    let roots = store.lock().toolbox_roots(project)?;
    Ok(Json(
        roots
            .iter()
            .map(|root| toolbox::scan(std::path::Path::new(root)))
            .collect(),
    ))
}

async fn history(
    State(state): State<AppState>,
    Path(project): Path<i64>,
) -> Result<Json<Vec<toolbox::Record>>> {
    let store = state.store()?;
    let records = store.lock().toolbox_history(project)?;
    Ok(Json(records))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    root: String,
    selection: Selection,
}

async fn preview(
    State(state): State<AppState>,
    Path(project): Path<i64>,
    Json(request): Json<Request>,
) -> Result<Json<Preview>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(toolbox::preview(
        &mut store,
        project,
        &request.root,
        request.selection,
    )?))
}

async fn saved(
    State(state): State<AppState>,
    Path((project, id)): Path<(i64, i64)>,
) -> Result<Json<Preview>> {
    let store = state.store()?;
    let result = store.lock().toolbox_preview(project, id)?;
    Ok(Json(result))
}

async fn apply(
    State(state): State<AppState>,
    Path((project, id)): Path<(i64, i64)>,
) -> Result<Json<Preview>> {
    let store = state.store()?;
    let mut store = store.lock();
    Ok(Json(toolbox::apply(&mut store, project, id)?))
}
