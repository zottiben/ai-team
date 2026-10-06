//! The human approves a proposed checkout only after the old turn has settled.

use std::path::Path as FsPath;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::{error::Result, state::AppState};
use ai_team_core::chat_workspaces::{self, WorkspaceChoice, WorkspaceRequest};
use ai_team_core::planning::PlanActor;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chat-workspaces", get(choices))
        .route("/chats/{id}/workspace", get(requests).post(propose))
        .route("/chats/{id}/workspace/approve", post(approve))
        .route("/chats/{id}/workspace/cancel", post(cancel))
}

#[derive(Deserialize)]
struct ProjectQuery {
    project: String,
}
async fn choices(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<WorkspaceChoice>>> {
    let root = {
        let store = state.store()?;
        let store = store.lock();
        chat_workspaces::project_repository(&store, store.find_project(&query.project)?.id)?
    };
    Ok(Json(chat_workspaces::choices(&root).await?))
}
#[derive(Serialize)]
struct Requests {
    requests: Vec<WorkspaceRequest>,
}
async fn requests(State(state): State<AppState>, Path(chat): Path<i64>) -> Result<Json<Requests>> {
    Ok(Json(Requests {
        requests: state.store()?.lock().chat_workspace_requests(chat)?,
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Propose {
    path: String,
}
async fn propose(
    State(state): State<AppState>,
    Path(chat): Path<i64>,
    Json(input): Json<Propose>,
) -> Result<Json<WorkspaceRequest>> {
    let root = chat_workspaces::repository(&state.store()?.lock(), chat)?;
    let path = chat_workspaces::validate(&root, FsPath::new(&input.path)).await?;
    Ok(Json(state.store()?.lock().request_chat_workspace(
        chat,
        PlanActor::Human,
        &path,
    )?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    request: i64,
    revision: i64,
}
async fn approve(
    State(state): State<AppState>,
    Path(chat): Path<i64>,
    Json(input): Json<Approval>,
) -> Result<Json<ai_team_core::Chat>> {
    let (root, proposal) = {
        let store = state.store()?;
        let store = store.lock();
        (
            chat_workspaces::repository(&store, chat)?,
            store.chat_workspace_request(chat, input.request)?,
        )
    };
    let target = chat_workspaces::validate(&root, FsPath::new(&proposal.to_path)).await?;
    Ok(Json(state.store()?.lock().apply_chat_workspace(
        chat,
        input.request,
        input.revision,
        &target,
    )?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    request: i64,
}
async fn cancel(
    State(state): State<AppState>,
    Path(chat): Path<i64>,
    Json(input): Json<Cancel>,
) -> Result<Json<serde_json::Value>> {
    state
        .store()?
        .lock()
        .cancel_chat_workspace(chat, input.request)?;
    Ok(Json(serde_json::json!({"cancelled":true})))
}
