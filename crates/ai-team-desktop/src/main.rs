//! ai-team, as a desktop app (D5).
//!
//! Tauri rather than Electron, for a structural reason rather than an aesthetic one:
//! the host process here *is* Rust, so it links `ai-team-core` and `ai-team-ui`
//! directly and there is no IPC boundary, no second runtime, and no second copy of the
//! routing rules in another language. It also uses the platform webview instead of
//! shipping Chromium, which is the difference between an app measured in tens of
//! megabytes and one measured in hundreds.
//!
//! The window is deliberately thin. It starts the same server `ait ui` starts and points
//! the webview at it, so there is one frontend, one API and one set of tests - the
//! desktop app cannot drift from the browser app because it *is* the browser app.

// No console window behind the app on Windows. Only in release: a debug build's stdout
// is how you find out why it did not start.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::mpsc;

use anyhow::{Context, Result};
use tauri::{WebviewUrl, WebviewWindowBuilder};

use ai_team_ui::{ServeOptions, Server};

fn main() {
    // The first thing that happens, and the reason this app could see any of the
    // operator's tools at all: launched from Finder or the Dock, this process inherits
    // launchd's `PATH=/usr/bin:/bin:/usr/sbin:/sbin` and cannot find `pi`, `aip`, `awt`,
    // `claude` or `codex` (D22). Before `run`, because that starts a runtime and
    // `set_var` is process-global.
    ai_team_core::adopt_login_path();

    if let Err(err) = run() {
        // A desktop app has nowhere to print, so a startup failure gets a dialog too.
        eprintln!("ai-team: {err:#}");
        alert(&format!("{err:#}"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // The server needs a runtime that outlives this function, and Tauri owns the main
    // thread for the event loop, so the runtime is leaked deliberately rather than
    // dropped at the end of `run` and taking the server with it.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    let runtime = Box::leak(Box::new(runtime));

    let (ready, started) = mpsc::channel();
    runtime.spawn(async move {
        // The same database the CLI writes, so the window and the terminal are two
        // views of one thing rather than two applications that agree by accident.
        let options = ServeOptions {
            store: ai_team_core::default_db_path()
                .ok()
                .and_then(|path| ai_team_core::Store::open(&path).ok()),
            // The window's Update button replaces the binary this server runs in, and
            // here that binary is the app's, not `ait`. Saying so is what keeps an
            // update from installing the CLI into `ai-team.app` and leaving an app that
            // launches, prints `--help` to nobody, and exits.
            host: ai_team_core::Host::Desktop,
            ..Default::default()
        };
        match Server::bind(options).await {
            Ok(server) => {
                let _ = ready.send(Ok(server.url()));
                let _ = server.serve().await;
            }
            Err(err) => {
                let _ = ready.send(Err(err.to_string()));
            }
        }
    });

    let url = started
        .recv()
        .context("the server did not start")?
        .map_err(|e| anyhow::anyhow!(e))?;
    eprintln!("ai-team: serving {url}");
    let url = url.parse().context("the server produced an unusable URL")?;

    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(move |app| {
            // Built here rather than in tauri.conf.json because the URL is not known
            // until the OS has assigned a port and the token has been minted.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("ai-team")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 560.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("running the desktop app")
}

/// Tauri's dialog plugin is not loaded yet when startup fails, so this uses the
/// platform's own facility and falls back to stderr where there is not one.
fn alert(message: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display dialog {} with title \"ai-team\" buttons {{\"OK\"}} with icon caution",
            applescript_string(message)
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = message;
    }
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
