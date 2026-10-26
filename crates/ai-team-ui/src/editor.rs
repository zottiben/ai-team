//! The editor's own routes: open with a fingerprint, preview, and a checked save.
//!
//! There is exactly one route that writes a file, and it is this one. The window cannot
//! reach an unchecked write, because there is no longer an unchecked write to reach - the
//! old `POST /file` is gone rather than left beside the safe one.
//!
//! A refused save is a **409 with a body**, not a 400 with a sentence. The window has to
//! branch on it - reload, compare, or keep the buffer - and parsing an error string to
//! decide that is how a refusal quietly becomes a crash.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ai_team_core::{FileRevision, SaveOutcome};

use crate::api::worktree_for;
use crate::error::Result;
use crate::state::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/editor/open", get(open))
        .route("/editor/baseline", get(baseline))
        .route("/editor/preview", post(preview))
        .route("/editor/save", post(save))
        .route("/editor/kept", get(kept))
        .route("/editor/kept/text", get(kept_text))
        .route("/editor/kept/discard", post(discard))
}

/// Which checkout, the same three ways every other editor route takes it.
#[derive(Debug, Deserialize)]
struct Scope {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
}

/// The same checkout, for the routes that are about the recovery store rather than one
/// file in the tree.
#[derive(Debug, Deserialize)]
struct Where {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
}

async fn open(
    State(state): State<AppState>,
    Query(query): Query<Scope>,
) -> Result<Json<ai_team_core::OpenedFile>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(ai_team_core::open_file(&worktree, &query.path)?))
}

#[derive(Debug, Serialize)]
struct Baseline {
    path: String,
    /// Null when HEAD does not have this path - a new file, or a repository with no
    /// commit. The window draws every line as added rather than drawing nothing.
    text: Option<String>,
    tracked: bool,
}

/// The committed text, for the change markers in the gutter.
async fn baseline(
    State(state): State<AppState>,
    Query(query): Query<Scope>,
) -> Result<Json<Baseline>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    let text = ai_team_core::file_at_head(&worktree, &query.path).await?;
    Ok(Json(Baseline {
        path: query.path,
        tracked: text.is_some(),
        text,
    }))
}

#[derive(Debug, Deserialize)]
struct PreviewRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
    /// The fingerprint the window got when it opened the file.
    base: FileRevision,
    text: String,
}

async fn preview(
    State(state): State<AppState>,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<ai_team_core::SavePreview>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    Ok(Json(ai_team_core::preview_save(
        &worktree,
        &request.path,
        &request.base,
        &request.text,
    )?))
}

#[derive(Debug, Deserialize)]
struct SaveRequest {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    path: String,
    base: FileRevision,
    /// The hash of the preview the operator confirmed. The service checks the body
    /// against it, so a confirmation cannot be replayed over different bytes.
    proposed_hash: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct Written {
    #[serde(flatten)]
    saved: ai_team_core::Saved,
    /// How many chats will be told about this edit when they next take a turn. Reported
    /// so the window can say "the agent will be told" only when that is true.
    notified_chats: usize,
}

async fn save(State(state): State<AppState>, Json(request): Json<SaveRequest>) -> Result<Response> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    let outcome = ai_team_core::save_file(
        &worktree,
        &request.path,
        &request.base,
        &request.proposed_hash,
        &request.text,
    )?;
    match outcome {
        SaveOutcome::Refused(refusal) => Ok((StatusCode::CONFLICT, Json(refusal)).into_response()),
        SaveOutcome::Written(saved) => {
            // Recorded after the write and only after it: a notice for a save that did
            // not happen would have the agent re-reading a file nobody touched.
            let notified_chats = {
                let store = state.store()?;
                let mut store = store.lock();
                store.record_editor_save(
                    &worktree.to_string_lossy(),
                    &saved.path,
                    saved.revision.bytes,
                )?
            };
            Ok(Json(Written {
                saved,
                notified_chats,
            })
            .into_response())
        }
    }
}

/// Every version the editor displaced in this checkout, newest first.
///
/// There is a route for this because a save that keeps the file it replaced and then
/// never says where is a save that has hidden the thing it was keeping.
async fn kept(
    State(state): State<AppState>,
    Query(query): Query<Where>,
) -> Result<Json<Vec<ai_team_core::PreservedEntry>>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(ai_team_core::preserved_files(&worktree)?))
}

/// A checkout and one name in its store.
///
/// The scope fields are spelled out rather than flattened: `Query` parses with
/// `serde_urlencoded`, which cannot flatten, and a route that only fails on the query
/// string is a route whose tests pass.
#[derive(Debug, Deserialize)]
struct Named {
    project: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    node: Option<i64>,
    /// A store-relative name from the listing. The core refuses anything that is not a
    /// single plain component, so this route cannot be pointed into the checkout.
    name: String,
}

#[derive(Debug, Serialize)]
struct KeptText {
    name: String,
    /// Null when there is no such kept file, which is not an error - the operator may
    /// have discarded it in another window.
    text: Option<ai_team_core::EditorText>,
}

async fn kept_text(
    State(state): State<AppState>,
    Query(query): Query<Named>,
) -> Result<Json<KeptText>> {
    let worktree = worktree_for(
        &state,
        &query.project,
        query.node,
        query.workspace.as_deref(),
    )
    .await?;
    Ok(Json(KeptText {
        text: ai_team_core::read_preserved_file(&worktree, &query.name)?,
        name: query.name,
    }))
}

#[derive(Debug, Serialize)]
struct Discarded {
    discarded: bool,
}

/// Throw one kept version away. The operator's call and only theirs: nothing on the save
/// path deletes a file it displaced.
async fn discard(
    State(state): State<AppState>,
    Json(request): Json<Named>,
) -> Result<Json<Discarded>> {
    let worktree = worktree_for(
        &state,
        &request.project,
        request.node,
        request.workspace.as_deref(),
    )
    .await?;
    Ok(Json(Discarded {
        discarded: ai_team_core::discard_preserved_file(&worktree, &request.name)?,
    }))
}
