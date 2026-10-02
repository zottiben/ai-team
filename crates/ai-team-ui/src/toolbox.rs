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
        .route("/toolbox/catalogue", get(catalogue))
        .route("/toolbox/user", get(user_scope))
        .route("/toolbox/user/preview", post(user_preview))
        .route("/toolbox/registry", get(registry))
        .route("/toolbox/registry/preview", post(registry_preview))
        .route("/toolbox/discover", post(discover))
        .route("/toolbox/operations/{kind}", get(operation_history))
        .route("/toolbox/operations/{kind}/{id}", get(operation))
        .route(
            "/toolbox/operations/{kind}/{id}/apply",
            post(operation_apply),
        )
        .route("/projects/{project}/toolbox/converge", post(converge))
        .route("/projects/{project}/toolbox", get(scan))
        .route("/projects/{project}/toolbox/history", get(history))
        .route("/projects/{project}/toolbox/preview", post(preview))
        .route("/projects/{project}/toolbox/previews/{id}", get(saved))
        .route(
            "/projects/{project}/toolbox/previews/{id}/apply",
            post(apply),
        )
}

async fn user_scope() -> Result<Json<toolbox::Authority>> {
    Ok(Json(toolbox::user_authority()?))
}
async fn user_preview(
    State(state): State<AppState>,
    Json(selection): Json<toolbox::UserSelection>,
) -> Result<Json<toolbox::Operation>> {
    let store = state.store()?;
    let result = toolbox::preview_user(&mut store.lock(), toolbox::user_authority()?, selection)?;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Converge {
    root: String,
    target: String,
}
async fn converge(
    State(state): State<AppState>,
    Path(project): Path<i64>,
    Json(request): Json<Converge>,
) -> Result<Json<toolbox::Operation>> {
    let store = state.store()?;
    let result =
        toolbox::preview_convergence(&mut store.lock(), project, &request.root, &request.target)?;
    Ok(Json(result))
}
async fn registry(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(
        serde_json::json!({"projects":store.toolbox_registrations()?,"roots":store.toolbox_scan_roots()?}),
    ))
}
async fn registry_preview(
    State(state): State<AppState>,
    Json(selection): Json<toolbox::RegistrySelection>,
) -> Result<Json<toolbox::Operation>> {
    let store = state.store()?;
    let result = toolbox::preview_registry(&mut store.lock(), selection)?;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Roots {
    roots: Vec<String>,
}
async fn discover(
    State(state): State<AppState>,
    Json(request): Json<Roots>,
) -> Result<Json<toolbox::Discovery>> {
    let store = state.store()?;
    let result = toolbox::discover(&store.lock(), request.roots)?;
    Ok(Json(result))
}
async fn operation_history(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<Json<Vec<toolbox::OperationRecord>>> {
    let store = state.store()?;
    let result = store.lock().toolbox_operations(&kind)?;
    Ok(Json(result))
}
async fn operation(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> Result<Json<toolbox::Operation>> {
    let store = state.store()?;
    let result = store.lock().toolbox_operation(id, &kind)?;
    Ok(Json(result))
}
async fn operation_apply(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> Result<Json<toolbox::Operation>> {
    let current = if kind == "user" {
        Some(toolbox::user_authority()?)
    } else {
        None
    };
    let store = state.store()?;
    let result = toolbox::apply_operation(&mut store.lock(), id, &kind, current.as_ref())?;
    Ok(Json(result))
}

async fn catalogue() -> Result<Json<toolbox::Catalogue>> {
    Ok(Json(toolbox::catalogue()?))
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
