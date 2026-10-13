//! Configuring the MCP servers AI Team gives its own agents.
//!
//! Every route here is a view over the store, like the rest of this crate. Two
//! properties it holds that are easy to lose:
//!
//! - **Reading runs nothing.** Listing servers, and listing what could be imported from
//!   the operator's own Pi definitions, read rows and files. No command is executed, no
//!   endpoint is reached and no credential is resolved, so a GET cannot become a shell
//!   on loopback by another name.
//! - **No stored value is returned.** The list and the import preview carry the keys of
//!   an `env` or `headers` map and never the values, and a save that omits them retains
//!   what is stored - which is the only shape that works when the page was never given
//!   them in the first place.
//! - **An import is the preview, approved.** The preview answers with a fingerprint of
//!   the scope and of what the operator's files held; the import quotes it back, and
//!   anything else is refused. A POST carrying names alone would be a second read of
//!   files that may have moved between the list somebody reviewed and the list that
//!   arrives.

use ai_team_core::{McpImportPreview, McpServer, McpServerEdit, NewMcpServer};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::{error::Result, state::AppState};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/mcp/servers", get(servers).post(add))
        .route("/mcp/servers/{id}", post(edit))
        .route("/mcp/servers/{id}/remove", post(remove))
        .route("/mcp/import", get(candidates).post(import))
}

/// Which scope a request is about. Absent is the app defaults alone.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    #[serde(default)]
    project: Option<i64>,
}

async fn servers(
    State(state): State<AppState>,
    Query(scope): Query<Scope>,
) -> Result<Json<Vec<McpServer>>> {
    let store = state.store()?;
    let servers = store.lock().mcp_servers(scope.project)?;
    Ok(Json(servers))
}

async fn add(
    State(state): State<AppState>,
    Json(new): Json<NewMcpServer>,
) -> Result<Json<McpServer>> {
    let store = state.store()?;
    let added = store.lock().add_mcp_server(new)?;
    Ok(Json(added))
}

async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(edit): Json<McpServerEdit>,
) -> Result<Json<McpServer>> {
    let store = state.store()?;
    let edited = store.lock().edit_mcp_server(id, edit)?;
    Ok(Json(edited))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Removal {
    rev: i64,
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(removal): Json<Removal>,
) -> Result<Json<serde_json::Value>> {
    let store = state.store()?;
    store.lock().remove_mcp_server(id, removal.rev)?;
    Ok(Json(serde_json::json!({ "removed": id })))
}

/// What the operator's own Pi definitions hold, for review, and the fingerprint that
/// pins an import to this answer.
///
/// Offered, never applied: nothing is imported because it happened to be there, and a
/// definition ai-team cannot carry across says so rather than arriving changed. Reading
/// opens no connection, runs no command and resolves no credential - the preview is
/// bounded reads of two named files.
#[derive(Clone)]
pub(crate) struct Sources(pub Vec<std::path::PathBuf>);

async fn candidates(
    axum::Extension(Sources(sources)): axum::Extension<Sources>,
    State(state): State<AppState>,
    Query(scope): Query<Scope>,
) -> Result<Json<McpImportPreview>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(ai_team_core::mcp_import_preview_at(
        &sources,
        &store,
        scope.project,
    )?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    #[serde(default)]
    project_id: Option<i64>,
    names: Vec<String>,
    /// The preview this was approved against. Required: a request without one has not
    /// said what it saw, and the store refuses it rather than re-reading the files.
    fingerprint: String,
}

async fn import(
    axum::Extension(Sources(sources)): axum::Extension<Sources>,
    State(state): State<AppState>,
    Json(request): Json<Import>,
) -> Result<Json<Vec<McpServer>>> {
    let store = state.store()?;
    let imported = ai_team_core::import_mcp_servers_at(
        &sources,
        &mut store.lock(),
        request.project_id,
        &request.names,
        &request.fingerprint,
    )?;
    Ok(Json(imported))
}
