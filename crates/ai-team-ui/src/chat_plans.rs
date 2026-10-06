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

#[derive(serde::Serialize)]
struct View {
    #[serde(flatten)]
    plan: ChatPlan,
    archived: bool,
    frozen: bool,
}
fn view(store: &ai_team_core::Store, plan: ChatPlan) -> Result<View> {
    let chat = store.chat(plan.chat_id)?;
    let frozen = chat
        .active_node_id
        .map(|node| {
            store
                .node_run(node)
                .and_then(|node| store.chat_team_run(node.run_id))
        })
        .transpose()?
        .flatten()
        .is_some_and(|team| team.approved_revision.is_some());
    Ok(View {
        plan,
        archived: chat.archived,
        frozen,
    })
}
async fn read(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<View>> {
    let store = state.store()?;
    let store = store.lock();
    Ok(Json(view(&store, store.chat_plan(id, PlanActor::Human)?)?))
}

async fn change(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(action): Json<PlanAction>,
) -> Result<Json<View>> {
    let store = state.store()?;
    let mut store = store.lock();
    let plan = store.change_chat_plan(id, PlanActor::Human, action)?;
    Ok(Json(view(&store, plan)?))
}
