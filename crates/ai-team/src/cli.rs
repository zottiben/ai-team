//! The command surface.
//!
//! Kept in one file so the whole CLI is readable at a glance. The orchestrator, team and
//! run commands land with the milestones that build them.

use clap::{Parser, Subcommand};

use ai_team_core::{OnFailure, ProjectKind, Provider, Reasoning};

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
    /// Bring a ClickUp ticket or epic in as a project.
    Ingest(IngestArgs),
    /// Configure the team: its seats, their models, zones and tool rules.
    #[command(subcommand)]
    Team(TeamCommand),
    /// Configure the seats on a team.
    #[command(subcommand)]
    Agents(AgentsCommand),
    /// Run one prompt through a supervised agent.
    Run(RunArgs),
    /// What to work on now, across every project. Ranked, not just listed.
    Today,
    /// Which agent-model pairing is earning its seat.
    Stats(StatsArgs),
    /// Run the scheduler without the window, so reminders fire when ai-team is closed.
    Daemon,
    /// Reminders, the idea inbox, and runs scheduled for later.
    #[command(subcommand)]
    Remind(RemindCommand),
    /// Replace this binary with the latest release.
    Update {
        /// Say what is available without installing it.
        #[arg(long)]
        check: bool,
    },
    /// Check the install: paths, the machine profile, and the embedded bundle.
    Doctor,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum RemindCommand {
    /// What is scheduled.
    Ls,
    /// A one-off or recurring reminder.
    Add(RemindArgs),
    /// Something to think about later. No date needed - that is what makes it an inbox.
    Idea(RemindArgs),
    /// Start a run unattended at a due time.
    Run(RemindArgs),
    /// Cancel one.
    Rm { id: i64 },
}

#[derive(Debug, clap::Args)]
pub(crate) struct RemindArgs {
    pub(crate) title: String,
    /// When, as RFC3339.
    #[arg(long)]
    pub(crate) at: Option<String>,
    /// When, relative: `30m`, `2h`, `1d`. A bare number is minutes.
    #[arg(long)]
    pub(crate) r#in: Option<String>,
    /// daily | weekdays | weekly | monthly
    #[arg(long)]
    pub(crate) every: Option<String>,
    /// Which project. A scheduled run defaults to the checkout you are standing in.
    #[arg(long, short)]
    pub(crate) project: Option<String>,
    /// What to build, for a scheduled run. Omit to build whatever the plan has ready.
    #[arg(long)]
    pub(crate) prompt: Option<String>,
    /// Anything worth remembering alongside it.
    #[arg(long)]
    pub(crate) note: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct StatsArgs {
    /// How to group the numbers.
    #[arg(long, default_value = "agent", value_parser = parse_by)]
    pub(crate) by: ai_team_core::By,
    /// Only this project. Defaults to every one.
    #[arg(long, short)]
    pub(crate) project: Option<String>,
}

fn parse_by(value: &str) -> std::result::Result<ai_team_core::By, String> {
    match value {
        "agent" => Ok(ai_team_core::By::Agent),
        "model" => Ok(ai_team_core::By::Model),
        "team" => Ok(ai_team_core::By::Team),
        "project" => Ok(ai_team_core::By::Project),
        other => Err(format!(
            "unknown grouping `{other}` - try agent, model, team or project"
        )),
    }
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
pub(crate) struct IngestArgs {
    /// The ClickUp task or list URL, or a Figma file URL.
    pub(crate) url: String,

    /// What to call it. Defaults to the id in the URL, until a seat with the connection
    /// reads the real title.
    #[arg(long)]
    pub(crate) name: Option<String>,

    /// The ticket's description, if you have it to hand. Any Figma links in it become
    /// design context for the seat that owns the UI.
    #[arg(long)]
    pub(crate) brief_file: Option<String>,
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

    /// Run one turn in this directory instead of planning and dispatching.
    ///
    /// Without it, the orchestrator writes a plan and each ready slice is built in its
    /// own leased worktree.
    #[arg(long)]
    pub(crate) worktree: Option<String>,

    /// Which ai-planner plan to work on. Defaults to the one this checkout resolves to.
    #[arg(long)]
    pub(crate) plan: Option<String>,

    /// How many slices may build at once. Defaults to the team's parallel width.
    #[arg(long)]
    pub(crate) width: Option<usize>,

    /// Write the plan, then stop before building anything.
    #[arg(long)]
    pub(crate) plan_only: bool,

    /// Plan again even though the board already has work ready to build.
    #[arg(long)]
    pub(crate) replan: bool,

    /// Work on the default branch itself instead of a fresh branch off it.
    ///
    /// A run normally puts its checkout on a new `ai-team/run-<id>` branch cut from
    /// origin's default branch, and never lands work on main/master. This is the one way
    /// past that, for this run only, from a checkout that has the default branch checked
    /// out.
    #[arg(long)]
    pub(crate) on_default_branch: bool,

    /// What to ask it to do. Leave it out to build whatever the plan already has ready.
    pub(crate) prompt: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TeamCommand {
    /// List the teams, and the projects running them.
    Ls,
    /// Show one team: its guardrails and every seat on it.
    Show {
        /// A project, or a team's own slug. Defaults to the current directory's project.
        team: Option<String>,
    },
    /// Change a team's name, description or guardrails.
    Edit(TeamEditArgs),
    /// Copy a team, its seats and their tool rules onto another project.
    Clone {
        /// The team to copy: a project running it, or a template's slug.
        #[arg(long)]
        from: String,
        /// The project to put the copy on. It becomes that project's team.
        #[arg(long)]
        to: String,
        /// What to call it. Defaults to the destination project's name.
        #[arg(long)]
        name: Option<String>,
        /// Delete the team the destination was running instead of leaving it behind.
        #[arg(long)]
        replace: bool,
    },
    /// Delete a team and its seats. The project it ran keeps its runs and its history.
    Rm {
        /// A project, or a team's own slug.
        team: String,
        /// Required: this removes every seat on the team.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, clap::Args)]
pub(crate) struct TeamEditArgs {
    /// A project, or a team's own slug. Defaults to the current directory's project.
    pub(crate) team: Option<String>,

    #[arg(long)]
    pub(crate) name: Option<String>,

    #[arg(long)]
    pub(crate) description: Option<String>,

    /// How many seats may work at once.
    #[arg(long)]
    pub(crate) parallel_width: Option<i64>,

    /// Token ceiling for a whole run. 0 removes it.
    #[arg(long)]
    pub(crate) budget_tokens_run: Option<i64>,

    /// Token ceiling for one seat's turn. 0 removes it.
    #[arg(long)]
    pub(crate) budget_tokens_node: Option<i64>,

    /// Wall-clock ceiling for a whole run, in seconds. 0 removes it.
    #[arg(long)]
    pub(crate) budget_seconds_run: Option<i64>,

    /// Wall-clock ceiling for one seat's turn, in seconds. 0 removes it.
    #[arg(long)]
    pub(crate) budget_seconds_node: Option<i64>,

    /// How many model turns one seat may take. 0 removes the cap.
    #[arg(long)]
    pub(crate) max_turns_node: Option<i64>,

    /// How many times a failed seat may be repaired before `on-failure` applies.
    #[arg(long)]
    pub(crate) max_repairs: Option<i64>,

    /// One of: retry, escalate, abort_branch.
    #[arg(long, value_parser = parse_on_failure)]
    pub(crate) on_failure: Option<OnFailure>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// Print where a project's Pi support files live: the guard, and one MCP config
    /// per seat.
    Path {
        #[arg(long, short)]
        project: String,
    },
    /// List the seats on a project's team.
    Ls {
        /// Which project. Slug, id, or part of the name.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Show one seat in full: model, zone, prompt and tool rules.
    Show {
        #[arg(long, short)]
        project: Option<String>,
        /// Which seat, by role.
        role: String,
    },
    /// Add a seat to the team.
    Add(AgentAddArgs),
    /// Change a seat. Only the flags you pass are touched.
    Edit(AgentEditArgs),
    /// Remove a seat. Its finished runs keep the role they recorded.
    Rm {
        #[arg(long, short)]
        project: Option<String>,
        role: String,
        /// Required: a removed seat cannot be undone.
        #[arg(long)]
        yes: bool,
    },
    /// Take a seat out of the roster without deleting it.
    Disable {
        #[arg(long, short)]
        project: Option<String>,
        role: String,
    },
    /// Put a disabled seat back.
    Enable {
        #[arg(long, short)]
        project: Option<String>,
        role: String,
    },
    /// Show or change one seat's tool rules. Deny always beats allow.
    Tools(AgentToolsArgs),
    /// Point one seat at a different model. Shorthand for `ait agents edit`.
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
        /// Its context window in tokens, when the model needs it stated.
        #[arg(long)]
        context_window: Option<i64>,
    },
}

#[derive(Debug, clap::Args)]
pub(crate) struct AgentAddArgs {
    #[arg(long, short)]
    pub(crate) project: Option<String>,

    /// The seat's role. A lowercase slug, unique on the team.
    #[arg(long)]
    pub(crate) role: String,

    /// Start from a built-in preset of the same or another role.
    #[arg(long)]
    pub(crate) preset: Option<String>,

    /// How it is addressed in the UI. Defaults to the role, or the preset's name.
    #[arg(long)]
    pub(crate) name: Option<String>,

    /// One or two sentences on what this seat is for. It reaches the model.
    #[arg(long)]
    pub(crate) purpose: Option<String>,

    /// One of: claude, openai, zai, local.
    #[arg(long, value_parser = parse_provider)]
    pub(crate) provider: Option<Provider>,

    #[arg(long)]
    pub(crate) model: Option<String>,

    /// One of: none, low, medium, high.
    #[arg(long, value_parser = parse_reasoning)]
    pub(crate) reasoning: Option<Reasoning>,

    /// A path glob this seat owns. Repeatable.
    #[arg(long)]
    pub(crate) zone: Vec<String>,

    /// Its context window in tokens. Defaults to the provider's registry default.
    #[arg(long)]
    pub(crate) context_window: Option<i64>,

    /// Replace the preset prompt with this file's contents.
    #[arg(long)]
    pub(crate) prompt_file: Option<String>,

    /// A checker: generate it no write_file or edit_file tools at all.
    #[arg(long)]
    pub(crate) read_only: bool,

    /// Where it sorts among the seats. Defaults to the end.
    #[arg(long)]
    pub(crate) ord: Option<i64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AgentEditArgs {
    #[arg(long, short)]
    pub(crate) project: Option<String>,

    /// Which seat, by its current role.
    pub(crate) role: String,

    /// Rename the role itself.
    #[arg(long)]
    pub(crate) new_role: Option<String>,

    #[arg(long)]
    pub(crate) name: Option<String>,

    #[arg(long)]
    pub(crate) purpose: Option<String>,

    #[arg(long, value_parser = parse_provider)]
    pub(crate) provider: Option<Provider>,

    #[arg(long)]
    pub(crate) model: Option<String>,

    #[arg(long, value_parser = parse_reasoning)]
    pub(crate) reasoning: Option<Reasoning>,

    /// Replace the owned paths. Repeatable; pass once with an empty string to clear.
    #[arg(long)]
    pub(crate) zone: Vec<String>,

    #[arg(long)]
    pub(crate) context_window: Option<i64>,

    /// Replace the prompt with this file's contents.
    #[arg(long)]
    pub(crate) prompt_file: Option<String>,

    /// Go back to a built-in preset's prompt.
    #[arg(long, conflicts_with = "prompt_file")]
    pub(crate) preset: Option<String>,

    #[arg(long, conflicts_with = "writes")]
    pub(crate) read_only: bool,

    /// Undo --read-only: generate the editing tools again.
    #[arg(long)]
    pub(crate) writes: bool,

    #[arg(long)]
    pub(crate) ord: Option<i64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct AgentToolsArgs {
    #[arg(long, short)]
    pub(crate) project: Option<String>,

    pub(crate) role: String,

    /// Allow a tool name or glob. Repeatable.
    #[arg(long)]
    pub(crate) allow: Vec<String>,

    /// Deny a tool name or glob. Repeatable, and it wins over any allow.
    #[arg(long)]
    pub(crate) deny: Vec<String>,

    /// Drop a rule entirely, so the caller's default decides again. Repeatable.
    #[arg(long)]
    pub(crate) clear: Vec<String>,

    /// Why, recorded next to the rule.
    #[arg(long)]
    pub(crate) note: Option<String>,
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

fn parse_reasoning(value: &str) -> Result<Reasoning, String> {
    value
        .parse()
        .map_err(|e: ai_team_core::Error| e.to_string())
}

fn parse_on_failure(value: &str) -> Result<OnFailure, String> {
    value
        .parse()
        .map_err(|e: ai_team_core::Error| e.to_string())
}
