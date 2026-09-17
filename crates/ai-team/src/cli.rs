//! The command surface.
//!
//! Kept in one file so the whole CLI is readable at a glance. `M0-S1` ships the two
//! commands that prove an install worked; the orchestrator, the team and the run
//! commands land with the milestones that build them.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ait",
    version,
    about = "Run a team of AI agents against your projects",
    long_about = None,
    propagate_version = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Open the ai-team window in a browser.
    Ui(UiArgs),
    /// Check the install: paths, the machine profile, and the embedded bundle.
    Doctor,
}

#[derive(Debug, clap::Args)]
pub(crate) struct UiArgs {
    /// Port to bind on loopback. 0 lets the OS pick a free one.
    #[arg(long, default_value_t = 0)]
    pub(crate) port: u16,

    /// Print the URL instead of opening a browser.
    #[arg(long)]
    pub(crate) no_open: bool,
}
