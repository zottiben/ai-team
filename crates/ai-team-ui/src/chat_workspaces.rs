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
        .route("/chat-workspace-setups", get(setups).post(setup))
        .route("/chat-workspace-setups/{id}", post(setup_command))
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
    let db = state.store()?.lock().path().to_path_buf();
    Ok(Json(chat_workspaces::owned_choices(&db, &root).await?))
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
    let db = state.store()?.lock().path().to_path_buf();
    let path = chat_workspaces::validate_owned(&db, &root, FsPath::new(&input.path)).await?;
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
    let db = state.store()?.lock().path().to_path_buf();
    let target =
        chat_workspaces::validate_owned(&db, &root, FsPath::new(&proposal.to_path)).await?;
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

async fn setups(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ai_team_core::workspace_setup::Setup>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(store.workspace_setups(
        store.find_project(&query.project)?.id,
    )?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupInput {
    project: String,
    request_id: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    approved: bool,
}
async fn setup(
    State(state): State<AppState>,
    Json(input): Json<SetupInput>,
) -> Result<Json<ai_team_core::workspace_setup::Setup>> {
    if !input.approved {
        return Err(ai_team_core::Error::invalid(
            "approve AWT pool reuse, hooks and dependency setup before starting",
        )
        .into());
    }
    let (db, receipt, started) = {
        let store = state.store()?;
        let mut store = store.lock();
        let project = store.find_project(&input.project)?.id;
        let root = chat_workspaces::project_repository(&store, project)?;
        let (receipt, started) = store.request_workspace_setup(
            project,
            &root,
            &input.request_id,
            input.branch.trim(),
        )?;
        (store.path().to_path_buf(), receipt, started)
    };
    if started {
        let (project, id) = (receipt.project_id, receipt.id);
        tokio::spawn(async move {
            if let Err(error) = ai_team_core::workspace_setup::run(&db, project, id).await {
                eprintln!("AWT setup {id} retained for inspection: {error}");
            }
        });
    }
    Ok(Json(receipt))
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SetupAction {
    Inspect,
    RetryDependencies,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupCommand {
    project: String,
    revision: i64,
    action: SetupAction,
}
async fn setup_command(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<SetupCommand>,
) -> Result<Json<ai_team_core::workspace_setup::Setup>> {
    let (db, project) = {
        let store = state.store()?;
        let store = store.lock();
        (
            store.path().to_path_buf(),
            store.find_project(&input.project)?.id,
        )
    };
    let result = tokio::spawn(async move {
        match input.action {
            SetupAction::Inspect => {
                ai_team_core::workspace_setup::inspect(&db, project, id, input.revision).await
            }
            SetupAction::RetryDependencies => {
                ai_team_core::workspace_setup::retry_dependencies(&db, project, id, input.revision)
                    .await
            }
        }
    })
    .await
    .map_err(|error| {
        ai_team_core::Error::invalid(format!(
            "setup worker stopped; inspect its receipt: {error}"
        ))
    })??;
    Ok(Json(result))
}
