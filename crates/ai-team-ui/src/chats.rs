//! Chat-scoped commands. Never infer an execution target from the project's latest run.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::ActivityEvent;
use crate::error::{Error, Result};
use crate::state::AppState;
use ai_team_core::{Chat, ChatTurn, ModelRegistry, NewChat, NodeStatus, Provider, Reasoning};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats", get(list).post(create))
        .route("/chats/{id}", get(detail).patch(edit))
        .route("/chats/{id}/events", get(events))
        .route("/chats/{id}/messages", post(send))
        .route("/chats/{id}/stop", post(stop))
        .route("/chats/{id}/resume", post(resume))
}

#[derive(Deserialize)]
struct ProjectQuery {
    project: String,
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<Chat>>> {
    let store = state.store()?;
    let store = store.lock();
    let project = store.find_project(&query.project)?;
    Ok(Json(store.chats(project.id)?))
}

#[derive(Deserialize)]
struct Create {
    project: String,
    workspace: Option<String>,
    provider: Provider,
    model: String,
    reasoning: Reasoning,
}

async fn create(State(state): State<AppState>, Json(input): Json<Create>) -> Result<Json<Chat>> {
    let workspace =
        crate::api::worktree_for(&state, &input.project, None, input.workspace.as_deref()).await?;
    let store = state.store()?;
    let mut store = store.lock();
    let project = store.find_project(&input.project)?;
    Ok(Json(store.create_chat(NewChat {
        project_id: project.id,
        workspace,
        provider: input.provider,
        model: input.model,
        reasoning: input.reasoning,
    })?))
}

#[derive(Serialize)]
struct Detail {
    #[serde(flatten)]
    chat: Chat,
    turns: Vec<ChatTurn>,
    state: &'static str,
    can_resume: bool,
    orphan_running: bool,
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Detail>> {
    let store = state.store()?;
    let store = store.lock();
    let chat = store.chat(id)?;
    let turns = store.chat_turns(id)?;
    let active = chat
        .active_node_id
        .map(|node| store.node_run(node))
        .transpose()?;
    let supervised = active
        .as_ref()
        .is_some_and(|node| chat.supervisor_alive(node));
    let orphan_running = !supervised && active.as_ref().is_some_and(|node| chat.pi_alive(node));
    let status = match active {
        Some(_) if !supervised => "interrupted",
        Some(_) if chat.stop_requested => "stopping",
        Some(_) => "running",
        None => match turns.last().map(|turn| turn.node.status) {
            None => "empty",
            Some(NodeStatus::Failed) => "failed",
            Some(NodeStatus::Cancelled) => "stopped",
            _ => "idle",
        },
    };
    Ok(Json(Detail {
        chat,
        turns,
        state: status,
        can_resume: status == "interrupted" && !orphan_running,
        orphan_running,
    }))
}

#[derive(Deserialize)]
struct Edit {
    title: Option<String>,
    archived: Option<bool>,
}

async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Edit>,
) -> Result<Json<Chat>> {
    let store = state.store()?;
    let mut store = store.lock();
    if input.title.is_some() == input.archived.is_some() {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "change either the title or archived state",
        )));
    }
    if let Some(title) = input.title {
        return Ok(Json(store.rename_chat(id, &title)?));
    }
    Ok(Json(
        store.archive_chat(id, input.archived.unwrap_or(false))?,
    ))
}

#[derive(Deserialize)]
struct Events {
    #[serde(default)]
    after: i64,
}

async fn events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<Events>,
) -> Result<Json<Vec<ActivityEvent>>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(
        store
            .chat_events(id, query.after, 500)?
            .into_iter()
            .map(ActivityEvent::from)
            .collect(),
    ))
}

#[derive(Deserialize)]
struct Message {
    message: String,
    request_id: String,
}

async fn send(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Message>,
) -> Result<Json<ai_team_core::ChatSubmission>> {
    let db = state.database_path()?;
    let registry = ModelRegistry::load()?;
    let receipt =
        state
            .store()?
            .lock()
            .begin_chat_turn(id, &input.message, &input.request_id, &registry)?;
    if receipt.started {
        spawn(state.clone(), db, id, receipt.node_id, false);
    }
    Ok(Json(receipt))
}

fn spawn(state: AppState, db: std::path::PathBuf, chat_id: i64, node_id: i64, recovering: bool) {
    tokio::spawn(async move {
        // Retain a watcher even if the driver fails before opening its connection, or panics.
        let worker = tokio::spawn(async move {
            ai_team_core::drive_chat(&db, chat_id, node_id, recovering).await
        });
        let failure = match worker.await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(format!("The chat worker stopped unexpectedly: {error}")),
        };
        if let Some(error) = failure {
            eprintln!("chat {chat_id} turn {node_id}: {error}");
            let recorded = (|| -> Result<()> {
                state
                    .store()?
                    .lock()
                    .fail_chat_worker(chat_id, node_id, &error)?;
                Ok(())
            })();
            if let Err(recording) = recorded {
                eprintln!("chat {chat_id} turn {node_id}: could not reconcile worker failure: {recording}");
            }
        }
    });
}

#[derive(Deserialize)]
struct Target {
    node_id: i64,
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<Target>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let chat = store.chat(id)?;
    if chat.active_node_id != Some(target.node_id) {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "that turn is no longer active in this chat",
        )));
    }
    let node = store.node_run(target.node_id)?;
    if chat.supervisor_alive(&node) {
        store.request_chat_stop(id, target.node_id)?;
    } else {
        if chat.pi_alive(&node) {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "the orphaned Pi process is still running; its checkout cannot be released safely yet",
            )));
        }
        store.claim_chat_resume(id, node.id)?;
        store.finish_chat_turn(
            id,
            node.id,
            NodeStatus::Cancelled,
            Some("Interrupted turn stopped. Working files are kept."),
        )?;
    }
    Ok(Json(serde_json::json!({"stopping": true})))
}

async fn resume(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<Target>,
) -> Result<Json<serde_json::Value>> {
    let db = state.database_path()?;
    let node_id = {
        let store = state.store()?;
        let mut store = store.lock();
        if store.chat(id)?.active_node_id != Some(target.node_id) {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "that turn is no longer active in this chat",
            )));
        }
        store.claim_chat_resume(id, target.node_id)?
    };
    spawn(state.clone(), db, id, node_id, true);
    Ok(Json(serde_json::json!({"resumed": true})))
}
