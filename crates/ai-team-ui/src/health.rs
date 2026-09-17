//! `/api/health` - the one route that exists before there is a store.
//!
//! It is what the install script's smoke check and `ait doctor` both ask, so it reports
//! the facts that distinguish a working install from a confusing one: the version, and
//! whether a real frontend bundle was compiled in.

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::assets;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct Health {
    pub version: &'static str,
    pub bundle_embedded: bool,
    pub bundle_files: usize,
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn health() -> Json<Health> {
    Json(Health {
        version: ai_team_core::VERSION,
        bundle_embedded: assets::is_embedded(),
        bundle_files: assets::len(),
    })
}
