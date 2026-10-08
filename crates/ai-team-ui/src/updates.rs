//! Update checks/recovery stay reachable while ordinary mutations are fenced.
use crate::{error::Result, state::AppState};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

pub(crate) fn is_update(request: &Request) -> bool {
    matches!(
        request
            .uri()
            .path()
            .strip_prefix("/api")
            .unwrap_or(request.uri().path()),
        "/update" | "/update/inspect"
    )
}

pub(crate) async fn admit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response> {
    let _permit = if request.method().is_safe() || is_update(&request) {
        None
    } else {
        Some(state.updates().admit()?)
    };
    Ok(next.run(request).await)
}
