//! Human-only team commands. Every request carries the displayed controller identity.
use crate::{
    error::{Error, Result},
    state::AppState,
};
use ai_team_core::{
    ChatBuildApproval, ChatBuildClose, ChatBuildClosure, ChatBuildRecovery, ChatBuildResume,
    ChatBuildSlice, ChatBuildStart, ChatTeamPhase, ChatTeamRun, Store,
};
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats/{id}/mode", post(mode))
        .route("/chats/{id}/team/review", post(review))
        .route("/chats/{id}/team/approve", post(approve))
        .route("/chats/{id}/team/stop", post(stop))
        .route("/chats/{id}/team/recover", post(recover))
        .route("/chats/{id}/team/reconcile", post(reconcile))
        .route("/chats/{id}/team/continue", post(resume))
        .route("/chats/{id}/team/close", post(close))
}

#[derive(Serialize)]
pub(crate) struct Build {
    pub execution: ChatTeamRun,
    pub slices: Vec<ChatBuildSlice>,
    pub closure: Option<ChatBuildClosure>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mode {
    mode: ai_team_core::ChatMode,
    expect_revision: i64,
}
async fn mode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Mode>,
) -> Result<Json<ai_team_core::Chat>> {
    Ok(Json(state.store()?.lock().set_chat_mode(
        id,
        input.mode,
        input.expect_revision,
    )?))
}

fn target(store: &Store, chat: i64, target: &ChatBuildRecovery) -> Result<ChatTeamRun> {
    if chat != target.chat_id {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "the command belongs to another chat",
        )));
    }
    let execution = store
        .chat_team_run(target.run_id)?
        .filter(|run| {
            run.chat_id == chat
                && run.control_node_id == target.node_id
                && run.rev == target.expect_revision
        })
        .ok_or_else(|| {
            ai_team_core::Error::invalid("this team execution changed; refresh before acting")
        })?;
    let current = store.chat(chat)?;
    if current.archived || current.active_node_id != Some(target.node_id) {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "this team execution no longer owns the chat",
        )));
    }
    Ok(execution)
}

async fn review(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildRecovery>,
) -> Result<Json<ai_team_core::ChatBuildReview>> {
    let mut store = Store::open(&state.database_path()?)?;
    target(&store, id, &input)?;
    let reviewed = store.chat_build_review(id, input.node_id).await?;
    if reviewed.execution.rev != input.expect_revision {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "the team changed during review; refresh",
        )));
    }
    Ok(Json(reviewed))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    target: ChatBuildRecovery,
    approval: ChatBuildApproval,
}
async fn approve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Approval>,
) -> Result<Json<ChatBuildStart>> {
    let db = state.database_path()?;
    let mut store = Store::open(&db)?;
    target(&store, id, &input.target)?;
    if input.target.expect_revision != input.approval.expect_control_revision {
        return Err(Error::Core(ai_team_core::Error::invalid(
            "approval must match the reviewed controller",
        )));
    }
    let start = store
        .approve_chat_build(id, input.target.node_id, &input.approval)
        .await?;
    let receipt = start.clone();
    watch(db.clone(), id, start.run_id, async move {
        ai_team_core::drive_chat_team_build(&db, receipt).await
    });
    Ok(Json(start))
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildRecovery>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    let mut store = store.lock();
    let execution = target(&store, id, &input)?;
    if execution.quiescent && execution.approved_revision.is_none() {
        store.stop_chat_team_planning(id, input.node_id, input.expect_revision)?;
    } else if execution.phase == ChatTeamPhase::Building
        && !execution.supervisor_alive()
        && store
            .chat_build_slices(input.run_id)?
            .iter()
            .all(|slice| slice.lease_state == "pending")
    {
        store.cancel_unstarted_chat_build(&ChatBuildStart {
            chat_id: id,
            run_id: input.run_id,
            node_id: input.node_id,
            revision: input.expect_revision,
        })?;
    } else {
        store.request_chat_team_stop(&input)?;
    }
    Ok(Json(serde_json::json!({"stopping":true})))
}

async fn recover(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildRecovery>,
) -> Result<Json<ChatTeamRun>> {
    target(&state.store()?.lock(), id, &input)?;
    let db = state.database_path()?;
    // Detach on a lost HTTP response rather than aborting a half-drained controller.
    task(tokio::spawn(async move {
        ai_team_core::recover_chat_team_processes(&db, &input).await
    }))
    .await
    .map(Json)
}

async fn reconcile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildRecovery>,
) -> Result<Json<ai_team_core::ChatBuildRecoveryReport>> {
    target(&state.store()?.lock(), id, &input)?;
    let db = state.database_path()?;
    task(tokio::spawn(async move {
        ai_team_core::reconcile_chat_team_build(&db, &input).await
    }))
    .await
    .map(Json)
}

async fn resume(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildResume>,
) -> Result<Json<serde_json::Value>> {
    {
        let store = state.store()?;
        let store = store.lock();
        let execution = target(&store, id, &input.target)?;
        if execution.phase != ChatTeamPhase::Blocked
            || !execution.quiescent
            || store.chat_build_closure(input.target.run_id)?.is_some()
        {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "continuation needs a drained, open retained build",
            )));
        }
        if !store
            .chat_build_slices(input.target.run_id)?
            .iter()
            .any(|slice| {
                slice.slice_key == input.slice_key && slice.rev == input.expect_slice_revision
            })
        {
            return Err(Error::Core(ai_team_core::Error::invalid(
                "the retained slice changed; refresh before continuing",
            )));
        }
    }
    let db = state.database_path()?;
    let run = input.target.run_id;
    let continuation = ai_team_core::ChatBuildContinuation::claim(&db, input)?;
    watch(db, id, run, continuation.run());
    Ok(Json(serde_json::json!({"accepted":true})))
}

async fn close(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChatBuildClose>,
) -> Result<Json<ai_team_core::ChatBuildCloseReport>> {
    target(&state.store()?.lock(), id, &input.target)?;
    let db = state.database_path()?;
    task(tokio::task::spawn_blocking(move || {
        ai_team_core::close_chat_team_build(&db, &input)
    }))
    .await
    .map(Json)
}

async fn task<T>(worker: tokio::task::JoinHandle<ai_team_core::Result<T>>) -> Result<T> {
    worker
        .await
        .map_err(|error| {
            ai_team_core::Error::invalid(format!(
                "team command stopped unexpectedly: {error}; refresh recovery state"
            ))
        })?
        .map_err(Error::from)
}

pub(crate) fn watch(
    db: std::path::PathBuf,
    chat: i64,
    run: i64,
    work: impl std::future::Future<Output = ai_team_core::Result<()>> + Send + 'static,
) {
    tokio::spawn(async move {
        if let Err(error) = task(tokio::spawn(work)).await {
            eprintln!("chat {chat} team worker: {error}");
            let recorded = Store::open(&db).and_then(|mut store| {
                store.append_event(
                    run,
                    ai_team_core::NewEvent::new(
                        ai_team_core::EventKind::Note,
                        format!("Team command ended without completion: {error}"),
                    )
                    .by("ai-team"),
                )
            });
            if let Err(recording) = recorded {
                eprintln!("chat {chat} team command evidence: {recording}");
            }
            // Never settle team ownership through the solo failure path. Discovery
            // preserves live workers and parks only provably abandoned controllers.
            if let Err(recovery) = ai_team_core::recover_abandoned_chat_team(&db, chat).await {
                eprintln!("chat {chat} team recovery: {recovery}");
            }
        }
    });
}
