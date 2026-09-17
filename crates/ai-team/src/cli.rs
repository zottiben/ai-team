//! The command surface.
//!
//! Kept in one file so the whole CLI is readable at a glance. The orchestrator, team and
//! run commands land with the milestones that build them.

use clap::{Parser, Subcommand};

use ai_team_core::{ProjectKind, Provider};

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
    /// Register this directory as a project and seed its team.
    Init(InitArgs),
    /// Open the ai-team window in a browser.
    Ui(UiArgs),
    /// Inspect the database directly.
    #[command(subcommand)]
    Db(DbCommand),
    /// Generate the eve project from the team rows.
    #[command(subcommand)]
    Agents(AgentsCommand),
    /// Run one prompt through a supervised agent.
    Run(RunArgs),
    /// Check the install: paths, the machine profile, and the embedded bundle.
    Doctor,
}

#[derive(Debug, clap::Args)]
pub(crate) struct InitArgs {
    /// What to call the project. Defaults to the repository or directory name.
    #[arg(long)]
    pub(crate) name: Option<String>,

    /// What kind of container this is. Defaults to `repo` inside a checkout.
    #[arg(long, value_parser = parse_kind)]
    pub(crate) kind: Option<ProjectKind>,
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

#[derive(Debug, clap::Args)]
pub(crate) struct RunArgs {
    /// Which project. Slug, id, or part of the name.
    #[arg(long, short)]
    pub(crate) project: String,

    /// The worktree the agent may act on. Defaults to the current directory.
    #[arg(long)]
    pub(crate) worktree: Option<String>,

    /// What to ask it to do.
    pub(crate) prompt: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// Rewrite the generated eve project from the current team rows.
    Generate {
        /// Which project. Slug, id, or part of the name.
        #[arg(long, short)]
        project: String,
        /// List what would be written without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print where a project's generated eve project lives.
    Path {
        #[arg(long, short)]
        project: String,
    },
    /// Point one seat at a different model. Full team editing is M2-S7.
    SetModel {
        #[arg(long, short)]
        project: String,
        /// Which seat, by role.
        #[arg(long)]
        role: String,
        /// One of: claude, openai, zai, local.
        #[arg(long, value_parser = parse_provider)]
        provider: Provider,
        /// The model name the provider knows it by.
        #[arg(long)]
        model: String,
        /// Its context window in tokens. eve refuses to compile compaction without one.
        #[arg(long)]
        context_window: Option<i64>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DbCommand {
    /// Print the path to the database file.
    Path,
    /// Open it in whatever this machine uses for .db files.
    Open,
    /// List the views that answer "what is going on" with no query written.
    Views,
}

/// clap's `ValueEnum` would need a second spelling of every variant, and the column
/// spelling in `ProjectKind` is already the canonical one.
fn parse_kind(value: &str) -> Result<ProjectKind, String> {
    value
        .parse()
        .map_err(|e: ai_team_core::Error| e.to_string())
}

fn parse_provider(value: &str) -> Result<Provider, String> {
    value
        .parse()
        .map_err(|e: ai_team_core::Error| e.to_string())
}
