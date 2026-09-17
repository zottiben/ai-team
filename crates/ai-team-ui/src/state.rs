//! What every handler is given.
//!
//! Cheap to clone by design - axum hands a copy to each request - so anything added
//! here belongs behind an `Arc`. `M0-S2` adds the store; today it carries the session
//! token and the paths `ait doctor` reports.

use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    token: String,
}

impl AppState {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: token.into(),
            }),
        }
    }

    pub fn token(&self) -> &str {
        &self.inner.token
    }
}

/// Hand-written so the token cannot reach a log through a derived `Debug`. The
/// workspace warns on missing `Debug`, and the lazy fix here would be the wrong one.
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}
