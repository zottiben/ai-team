//! Authenticated chat review and human-only delivery; never legacy run endpoints.
use ai_team_core::{
    chat_changes::{self, DeliveryAction, DraftTarget, Finding},
    Store,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::{error::Result, state::AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/chats/{id}/changes", get(changes))
        .route("/chats/{id}/checkout", get(checkout))
        .route("/chats/{id}/checkout/preview", post(checkout_preview))
        .route("/chats/{id}/checkout/finding", post(checkout_finding))
        .route("/chats/{id}/checkout/command", post(checkout_command))
        .route("/chats/{id}/draft/review", post(review))
        .route("/chats/{id}/draft/findings", post(finding))
        .route("/chats/{id}/draft/tree", post(tree))
        .route("/chats/{id}/draft/file", post(file))
        .route("/chats/{id}/retained/inspect", post(retained))
        .route("/chats/{id}/retained/file", post(retained_file))
        .route("/chats/{id}/retained/keep", post(keep))
        .route("/chats/{id}/delivery/preview", post(preview))
        .route("/chats/{id}/delivery/approve", post(approve))
        .route("/chats/{id}/delivery/inspect", post(inspect))
        .route("/chats/{id}/delivery/acknowledge", post(acknowledge))
}
async fn checkout(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<chat_changes::checkout::State>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(chat_changes::checkout::state(&mut store, id).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutPreview {
    fingerprint: String,
    action: chat_changes::checkout::Action,
}
async fn checkout_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<CheckoutPreview>,
) -> Result<Json<chat_changes::checkout::Operation>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::checkout::preview(&mut store, id, &input.fingerprint, input.action).await?,
    ))
}
async fn checkout_finding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<chat_changes::checkout::Finding>,
) -> Result<Json<serde_json::Value>> {
    let mut store = Store::open(&state.database_path()?)?;
    chat_changes::checkout::finding(&mut store, id, &input).await?;
    Ok(Json(serde_json::json!({"recorded":true})))
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum CheckoutCommand {
    Approve {
        id: i64,
        revision: i64,
    },
    Inspect {
        id: i64,
        revision: i64,
    },
    Acknowledge {
        id: i64,
        revision: i64,
        fingerprint: String,
        reason: String,
    },
}
async fn checkout_command(
    State(state): State<AppState>,
    Path(chat): Path<i64>,
    Json(input): Json<CheckoutCommand>,
) -> Result<Json<serde_json::Value>> {
    let db = state.database_path()?;
    let result = tokio::spawn(async move {
        use chat_changes::checkout as c;
        let mut store = Store::open(&db)?;
        Ok::<_, ai_team_core::Error>(match input {
            CheckoutCommand::Approve { id, revision } => {
                serde_json::to_value(c::approve(&mut store, chat, id, revision).await?)?
            }
            CheckoutCommand::Inspect { id, revision } => {
                serde_json::to_value(c::inspect(&mut store, chat, id, revision).await?)?
            }
            CheckoutCommand::Acknowledge {
                id,
                revision,
                fingerprint,
                reason,
            } => serde_json::to_value(
                c::acknowledge(&mut store, chat, id, revision, &fingerprint, &reason).await?,
            )?,
        })
    })
    .await
    .map_err(|e| {
        ai_team_core::Error::invalid(format!(
            "checkout command interrupted; inspect its receipt: {e}"
        ))
    })??;
    Ok(Json(result))
}
async fn changes(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<chat_changes::Changes>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(chat_changes::changes(&mut store, id).await?))
}
async fn review(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<DraftTarget>,
) -> Result<Json<chat_changes::DraftReview>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(chat_changes::review(&mut store, id, &target).await?))
}
async fn finding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Finding>,
) -> Result<Json<serde_json::Value>> {
    state.store()?.lock().review_chat_draft(id, &input)?;
    Ok(Json(serde_json::json!({"recorded":true})))
}
async fn tree(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<DraftTarget>,
) -> Result<Json<Vec<chat_changes::CommitEntry>>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::committed_tree(&mut store, id, &target).await?,
    ))
}
async fn file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<chat_changes::CommitFileRequest>,
) -> Result<Json<chat_changes::CommitFile>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::committed_file(&mut store, id, &input).await?,
    ))
}
async fn retained(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(target): Json<DraftTarget>,
) -> Result<Json<chat_changes::RetainedInspection>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::inspect_retained(&mut store, id, &target).await?,
    ))
}
async fn retained_file(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<chat_changes::RetainedFileRequest>,
) -> Result<Json<chat_changes::RetainedFile>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::retained_file(&mut store, id, &input).await?,
    ))
}
async fn keep(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<chat_changes::KeepRetained>,
) -> Result<Json<serde_json::Value>> {
    state.store()?.lock().keep_chat_retained_work(id, &input)?;
    Ok(Json(serde_json::json!({"kept_protected":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    target: DraftTarget,
    action: DeliveryAction,
}
async fn preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Preview>,
) -> Result<Json<chat_changes::Delivery>> {
    let mut store = Store::open(&state.database_path()?)?;
    Ok(Json(
        chat_changes::preview(&mut store, id, &input.target, input.action).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    delivery_id: i64,
    expect_revision: i64,
}
async fn approve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Approval>,
) -> Result<Json<chat_changes::Delivery>> {
    let db = state.database_path()?;
    // Dropping the HTTP request does not cancel an approved external operation. The
    // durable claim remains visible even if the server dies before it records a result.
    let task = tokio::spawn(async move {
        let mut store = Store::open(&db)?;
        chat_changes::approve(&mut store, id, input.delivery_id, input.expect_revision).await
    });
    let result = task.await.map_err(|error| {
        ai_team_core::Error::invalid(format!(
            "delivery task interrupted; inspect its recorded state: {error}"
        ))
    })??;
    Ok(Json(result))
}

async fn inspect(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<Approval>,
) -> Result<Json<chat_changes::DeliveryInspection>> {
    let db = state.database_path()?;
    let result = tokio::spawn(async move {
        let mut store = Store::open(&db)?;
        chat_changes::inspect(&mut store, id, input.delivery_id, input.expect_revision).await
    })
    .await
    .map_err(|error| {
        ai_team_core::Error::invalid(format!("delivery inspection interrupted: {error}"))
    })??;
    Ok(Json(result))
}
async fn acknowledge(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<chat_changes::DeliveryAcknowledgement>,
) -> Result<Json<chat_changes::DeliveryInspection>> {
    let db = state.database_path()?;
    let result = tokio::spawn(async move {
        let mut store = Store::open(&db)?;
        chat_changes::acknowledge(&mut store, id, &input).await
    })
    .await
    .map_err(|error| {
        ai_team_core::Error::invalid(format!("delivery acknowledgement interrupted: {error}"))
    })??;
    Ok(Json(result))
}
