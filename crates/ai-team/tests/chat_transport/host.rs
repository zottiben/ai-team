//! Opt-in, offline real-Pi + embedded-MCP acceptance. Never uses subscription models.
//! cargo run -p ai-team --example chat-transport -- --pi /path/to/pi --adapter /path/to/index.ts
mod auth_check;
mod context_failure;
mod fixture;
mod interruption;
mod scenario;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Options {
    /// An existing Pi executable. Nothing is installed by this fixture.
    #[arg(long)]
    pi: PathBuf,
    /// An existing pi-mcp-adapter entry point, loaded without altering its settings.
    #[arg(long)]
    adapter: PathBuf,
}

fn main() -> anyhow::Result<()> {
    // Same embedded entrypoint as both production binaries, including current_exe scope.
    if let Some(command) = ai_team_ui::plan_mcp::desktop_command() {
        return tokio::runtime::Runtime::new()?
            .block_on(command?.run())
            .map_err(Into::into);
    }
    let options = Options::parse();
    // Isolate process-global state before starting any worker threads.
    let mut fixture = fixture::Fixture::new(&options.pi, &options.adapter)?;
    eprintln!("Offline transport fixture: {}", fixture.root.display());
    let result = tokio::runtime::Runtime::new()?.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(240), scenario::run(&fixture))
            .await
            .map_err(anyhow::Error::from)?
    });
    result?;
    fixture.passed = true;
    println!("PASS: actual Pi, scoped embedded MCP, draft commit, solo/team/solo; no model network or installed awt");
    Ok(())
}
