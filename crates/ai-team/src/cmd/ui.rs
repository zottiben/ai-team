//! `ait ui` - the window, in a browser.

use anyhow::{Context, Result};

use ai_team_ui::{ServeOptions, Server};

use crate::cli::UiArgs;

pub(crate) async fn run(args: UiArgs) -> Result<()> {
    let server = Server::bind(ServeOptions {
        port: args.port,
        token: None,
    })
    .await
    .context("starting the local server")?;

    let url = server.url();
    println!("ai-team is at {url}");
    if !args.no_open {
        // Best effort. A browser that will not open is an annoyance, not a failure -
        // the URL is on stdout either way.
        open(&url);
    }
    // Ctrl-C is the documented way to stop it, so the shutdown path is the default one.
    println!("Ctrl-C to stop.");

    server.serve().await.context("serving the app")
}

fn open(url: &str) {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/C", "start", ""])
    } else {
        ("xdg-open", &[])
    };
    let _ = std::process::Command::new(program)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
