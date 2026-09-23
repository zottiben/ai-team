//! Server errors, and how they reach the browser.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Core(#[from] ai_team_core::Error),

    #[error("unauthorized")]
    Unauthorized,

    #[error("this server was started without a database - run `ait init` first")]
    NoStore,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            // Not an internal error: the server is working, there is just nothing for it
            // to show yet, and the window can say so instead of looking broken.
            Error::NoStore => StatusCode::SERVICE_UNAVAILABLE,
            Error::Core(ai_team_core::Error::Invalid(_)) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // A JSON body, always: the frontend reads one shape whether a call succeeded or
        // not, and a bare status line tells a developer nothing at 2am.
        let body = serde_json::json!({ "error": self.to_string() });
        (status, axum::Json(body)).into_response()
    }
}
