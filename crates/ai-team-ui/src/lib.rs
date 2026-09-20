//! The local server behind `ait ui` and the desktop window.
//!
//! One server, two front doors. The desktop shell starts this and points a webview at
//! it, so the window cannot drift from the browser app - it *is* the browser app. That
//! is also why there is no Tauri command layer: the API is HTTP, and both shells speak
//! it.
//!
//! `M0-S1` establishes the shape: bound to loopback, token-gated, serving the embedded
//! bundle and `/api/health`. The real routes arrive with the store.

mod api;
mod assets;
mod auth;
mod error;
mod health;
mod state;

use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use tokio::net::TcpListener;

pub use auth::{TOKEN_HEADER, TOKEN_QUERY};
pub use error::{Error, Result};
pub use health::Health;
pub use state::AppState;

/// What the binary was built with. `ait doctor` reports it, because a binary compiled
/// without a frontend bundle serves an explanation instead of an app, and that is worth
/// knowing before you go looking for the bug elsewhere.
pub fn bundle() -> Bundle {
    Bundle {
        embedded: assets::is_embedded(),
        files: assets::len(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Bundle {
    pub embedded: bool,
    pub files: usize,
}

// Not `Clone`: it owns a database connection, and two servers sharing one would be two
// writers behind one lock rather than the two processes SQLite already handles.
#[derive(Debug, Default)]
pub struct ServeOptions {
    /// 0 asks the OS for a free one, so two windows never fight over a number.
    pub port: u16,
    /// Supply one to keep a URL stable across restarts. Otherwise it is minted fresh.
    pub token: Option<String>,
    /// The database to read. Without one the window serves, reports its health, and says
    /// there is nothing to show - which is a better answer than refusing to start.
    pub store: Option<ai_team_core::Store>,
    /// Where to look for a database that does not exist yet.
    ///
    /// Named rather than resolved on demand so the server knows which database it serves.
    /// `None` means the machine's own, which is what `ait ui` passes; a test passes its
    /// own, because a test that reaches for the real one writes to the developer's home.
    pub db_path: Option<std::path::PathBuf>,
}

/// Bound, but not yet serving.
///
/// Split in two so a caller can print and open the real URL - which it cannot know
/// until the OS has assigned the port and the token has been minted - before it blocks
/// on [`Server::serve`].
#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
    token: String,
    listener: TcpListener,
    router: Router,
}

impl Server {
    pub async fn bind(options: ServeOptions) -> Result<Server> {
        let token = match options.token {
            Some(t) if !t.trim().is_empty() => t,
            _ => auth::mint_token()?,
        };
        let mut state = AppState::new(token.as_str()).watching(
            options
                .db_path
                .clone()
                .or_else(|| ai_team_core::default_db_path().ok()),
        );
        if let Some(store) = options.store {
            state = state.with_store(store);
        }

        let api = api::routes().route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ));

        let router = Router::new()
            .nest("/api", api)
            .fallback(assets::serve)
            .with_state(state);

        // Loopback only, and deliberately not configurable. This API will be able to
        // dispatch agents against real worktrees; it is not something to expose by flag.
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, options.port)))
            .await
            .map_err(|e| {
                Error::Io(std::io::Error::new(
                    e.kind(),
                    format!("binding 127.0.0.1:{}: {e}", options.port),
                ))
            })?;
        let addr = listener.local_addr()?;

        Ok(Server {
            addr,
            token,
            listener,
            router,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The URL to open. The token rides in the query because it is the only channel a
    /// freshly opened browser tab has.
    pub fn url(&self) -> String {
        format!("http://{}/?{}={}", self.addr, TOKEN_QUERY, self.token)
    }

    pub async fn serve(self) -> Result<()> {
        axum::serve(self.listener, self.router).await?;
        Ok(())
    }
}
