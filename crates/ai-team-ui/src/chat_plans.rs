//! Human planning commands target an explicit chat and share the MCP service contract.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::error::Result;
use crate::state::AppState;
use ai_team_core::planning::{ChatPlan, PlanAction, PlanActor};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/chats/{id}/plan", get(read).post(change))
}

async fn read(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<ChatPlan>> {
    Ok(Json(state.store()?.lock().chat_plan(id, PlanActor::Human)?))
}

async fn change(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(action): Json<PlanAction>,
) -> Result<Json<ChatPlan>> {
    Ok(Json(state.store()?.lock().change_chat_plan(
        id,
        PlanActor::Human,
        action,
    )?))
}
