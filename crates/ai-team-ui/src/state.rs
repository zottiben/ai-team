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
    /// One connection, shared, and openable later.
    ///
    /// SQLite in WAL handles concurrent *processes* fine, but a `Store` is not `Sync` and
    /// axum hands a clone of this to every request - so the handlers take turns rather
    /// than each opening their own.
    ///
    /// `Option` *inside* the lock rather than outside it, which is the whole point: the
    /// window can now be started before the database exists and pick it up afterwards.
    /// It used to decide at startup and never look again, so creating a database from the
    /// setup page left every route answering "run `ait init`" until the process was
    /// restarted - which is a confusing way to be told that setup worked.
    store: Mutex<Option<Store>>,
    /// Where to look for a database that is not open yet. Named rather than resolved on
    /// demand so this server knows which database it serves - and so a test can point at
    /// its own instead of the developer's home.
    db_path: Option<std::path::PathBuf>,
    /// The language servers this process has running. Shared rather than per-request:
    /// rust-analyzer takes the better part of a minute to index, so a server started for
    /// one request has to still be there for the next.
    lsp: ai_team_core::Pool,
    /// The terminals this process has open. A session outlives the window, so it lives
    /// here rather than in a request.
    terminals: ai_team_core::Terminals,
    /// Which program is serving. The update route replaces the binary it runs in, and
    /// `ait ui` and the desktop app are two different binaries in one release.
    host: ai_team_core::Host,
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
                store: Mutex::new(None),
                db_path: None,
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
                host: ai_team_core::Host::Cli,
            }),
        }
    }

    /// Which program is serving, for the update route.
    #[must_use]
    pub fn hosted_by(self, host: ai_team_core::Host) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: self.inner.token.clone(),
                store: Mutex::new(None),
                db_path: self.inner.db_path.clone(),
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
                host,
            }),
        }
    }

    /// Where to find a database that appears later.
    #[must_use]
    pub fn watching(self, db_path: Option<std::path::PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: self.inner.token.clone(),
                store: Mutex::new(None),
                db_path,
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
                host: self.inner.host,
            }),
        }
    }

    #[must_use]
    pub fn with_store(self, store: Store) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: self.inner.token.clone(),
                store: Mutex::new(Some(store)),
                db_path: self.inner.db_path.clone(),
                lsp: ai_team_core::Pool::new(),
                terminals: ai_team_core::Terminals::new(),
                host: self.inner.host,
            }),
        }
    }

    pub fn token(&self) -> &str {
        &self.inner.token
    }

    /// Which program is serving this window.
    pub(crate) fn host(&self) -> ai_team_core::Host {
        self.inner.host
    }

    /// The store, or a plain explanation.
    ///
    /// Opens one if there is not one yet, so a database created after the window started -
    /// by the setup page, or by `ait init` in another terminal - is picked up on the next
    /// request rather than on the next restart. A window served without one can still
    /// report its health, which is exactly the case being diagnosed.
    pub(crate) fn store(&self) -> Result<StoreGuard<'_>> {
        let mut slot = self.lock_slot();
        if slot.is_none() {
            // Opened, never created: creating a database is a repair somebody asked for
            // (D17), and a GET that silently brings one into being is a side effect
            // nobody can see.
            if let Some(store) = self
                .inner
                .db_path
                .as_ref()
                .filter(|path| path.exists())
                .and_then(|path| Store::open(path).ok())
            {
                *slot = Some(store);
            }
        }
        if slot.is_none() {
            return Err(Error::NoStore);
        }
        drop(slot);
        Ok(StoreGuard {
            store: &self.inner.store,
        })
    }

    fn lock_slot(&self) -> MutexGuard<'_, Option<Store>> {
        self.inner
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) struct StoreGuard<'a> {
    store: &'a Mutex<Option<Store>>,
}

impl StoreGuard<'_> {
    /// Recovers from a poisoned lock rather than propagating it: a panic in one handler
    /// must not take the window down for every later request.
    ///
    /// Unwrapping the `Option` is safe here because [`AppState::store`] returned `Err`
    /// unless it had filled it, and nothing ever puts it back to `None`.
    pub(crate) fn lock(&self) -> StoreRef<'_> {
        StoreRef(
            self.store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// A locked store that is known to be there.
pub(crate) struct StoreRef<'a>(MutexGuard<'a, Option<Store>>);

impl std::ops::Deref for StoreRef<'_> {
    type Target = Store;

    fn deref(&self) -> &Store {
        self.0.as_ref().expect("AppState::store filled this")
    }
}

impl std::ops::DerefMut for StoreRef<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        self.0.as_mut().expect("AppState::store filled this")
    }
}

/// Hand-written so the token cannot reach a log through a derived `Debug`. The
/// workspace warns on missing `Debug`, and the lazy fix here would be the wrong one.
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}
