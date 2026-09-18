//! What every handler is given.
//!
//! Cheap to clone by design - axum hands a copy to each request - so anything added
//! here belongs behind an `Arc`. `M0-S2` adds the store; today it carries the session
//! token and the paths `ait doctor` reports.

use std::sync::{Arc, Mutex, MutexGuard};

use ai_team_core::Store;

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    token: String,
    /// One connection, shared. SQLite in WAL handles concurrent *processes* fine, but a
    /// `Store` is not `Sync`, and axum hands a clone of this to every request - so the
    /// handlers take turns rather than each opening their own.
    store: Option<Mutex<Store>>,
    /// The language servers this process has running. Shared rather than per-request:
    /// rust-analyzer takes the better part of a minute to index, so a server started for
    /// one request has to still be there for the next.
    lsp: ai_team_core::Pool,
    /// The terminals this process has open. A session outlives the window, so it lives
    /// here rather than in a request.
    terminals: ai_team_core::Terminals,
}

impl AppState {
    /// The language server pool, for the routes that need one.
    pub fn lsp(&self) -> &ai_team_core::Pool {
        &self.inner.lsp
    }

    pub fn terminals(&self) -> &ai_team_core::Terminals {
        &self.inner.terminals
    }

    pub fn new(token: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: token.into(),
                store: None,
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
            }),
        }
    }

    #[must_use]
    pub fn with_store(self, store: Store) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: self.inner.token.clone(),
                store: Some(Mutex::new(store)),
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
            }),
        }
    }

    pub fn token(&self) -> &str {
        &self.inner.token
    }

    /// The store, or a plain explanation. A window served without one can still report
    /// its health, which is exactly the case `ait doctor` is diagnosing.
    pub(crate) fn store(&self) -> Result<StoreGuard<'_>> {
        let store = self.inner.store.as_ref().ok_or_else(|| Error::NoStore)?;
        Ok(StoreGuard { store })
    }
}

pub(crate) struct StoreGuard<'a> {
    store: &'a Mutex<Store>,
}

impl StoreGuard<'_> {
    /// Recovers from a poisoned lock rather than propagating it: a panic in one handler
    /// must not take the window down for every later request.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Store> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Hand-written so the token cannot reach a log through a derived `Debug`. The
/// workspace warns on missing `Debug`, and the lazy fix here would be the wrong one.
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}
