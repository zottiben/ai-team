//! The org graph, the store and the run state behind ai-team.
//!
//! This crate is where the things we need to see live: team and agent rows, routing,
//! budgets, run and step state (pillar 1). Everything above it - the CLI, the local
//! server, the desktop shell - is a presentation of what is in here, and none of them
//! keeps state of its own or writes SQL.
//!
//! What is deliberately *not* here is the work graph. Plans and slices belong to
//! ai-planner (D4); this crate only ever references them by their own keys, because a
//! local copy of a slice's title is a second source of truth that drifts.

mod analytics;
mod context;
mod daemon;
mod db;
mod diff;
mod error;
mod eve;
mod gates;
mod generate;
mod guardrails;
mod house;
mod lsp;
mod machine;
mod model;
mod neighbours;
mod paths;
mod review;
mod roles;
mod schedule;
mod store;
mod supervise;
mod today;
mod util;
mod workflow;

pub use analytics::{rollup, By, Row};
pub use context::{figma_links, parse_url, Source, CLICKUP_READ_TOOLS, FIGMA_READ_TOOLS};
pub use daemon::{once as tick_once, serve as serve_schedule};
pub use db::Db;
pub use diff::{parse as parse_diff, FileDiff, FileStatus, Hunk, Line, LineKind};
pub use error::{Error, Result};
pub use eve::{Disposition, Ingested, StreamEvent, StreamMeta, TerminalState};
pub use gates::{all_passed, discover_gates, evidence, run_gates, Gate, GateKind, GateResult};
pub use generate::{GeneratedFile, GeneratedProject, ModelExpression, ROOT_ROLE, VERIFIER_ROLE};
pub use guardrails::{node_may_continue, run_may_continue, Exceeded, Fallout};
pub use house::{read as read_house_rules, section as house_section, Rules};
pub use lsp::{
    language_for, language_id, workspace_root, Client, Diagnostic, Hover, Language, Location, Pool,
    Position, Range,
};
pub use machine::{
    ensure_machine_profile, ContextSource, MachineProfile, ModelRegistry, ModelResolution,
    ProviderState, ProviderStatus, DEFAULT_MACHINE_PROFILE,
};
pub use model::{
    Agent, CommentStatus, DiffSide, Event, EventKind, Guardrails, NewAgent, NewComment, NewEvent,
    NewProject, NewReminder, NewRepo, NodeRun, NodeStatus, OnFailure, Project, ProjectKind,
    ProjectRepo, ProjectSource, ProjectStatus, Provider, Reasoning, Recur, Reminder, ReminderKind,
    ReminderStatus, Review, ReviewComment, ReviewStatus, Run, RunStatus, RunTrigger, Team,
    ToolEffect, ToolPolicy, Usage,
};
pub use neighbours::{
    file_sql_available, list_tree, safe_join, Entry, FileSql, Hit, Lease, PlanSummary, Planner,
    PoolEntry, Question, Slice, Worktrees,
};
pub use paths::{data_dir, default_db_path, ensure_data_dir, machine_profile_path, HOME_ENV};
pub use review::{
    amendment, as_instructions, deliver as deliver_review, diff_for, instructions_for,
    pending as pending_review, responsible, steerable, Audience, Pending, Submitted,
};
pub use roles::{preset, RolePreset, DEFAULT_LOCAL_MODEL, DEFAULT_ROSTER};
pub use schedule::{act, announcement, claim_due, notify, runnable, tick, Fired, TICK};
pub use store::Store;
pub use supervise::{
    approvals_in, drive_turn, free_port, mint_token, outcome_status, reattach,
    record_build_progress, run_turn, AgentInfo, Approval, ApprovalOption, BuildPhase,
    BuildProgress, Dispatched, EveClient, EveEnv, EveProcess, Flow, Orchestration, Orchestrator,
    ProgressLine, Supervisor, TurnOutcome,
};
pub use today::{from_question, from_store as today_from_store, rank as rank_today, Item, Urgency};
pub use util::{normalise_remote, now, rfc3339_in, slugify, zone_matches};
pub use workflow::{run as run_workflow, Progress, Request};

/// The workspace version, compiled in.
///
/// Read from the manifest rather than written out again, because a version constant that
/// has to be kept in sync by hand is a version constant that is wrong.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
