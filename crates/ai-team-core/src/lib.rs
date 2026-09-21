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
mod crew;
mod daemon;
mod db;
mod diff;
mod error;
mod gates;
mod guardrails;
mod house;
mod lsp;
mod machine;
mod map;
mod model;
mod neighbours;
mod onboard;
mod paths;
mod pi;
mod readiness;
mod review;
mod roles;
mod schedule;
mod speak;
mod store;
mod supervise;
mod terminal;
mod today;
mod update;
mod util;
mod workflow;

pub use analytics::{rollup, By, Row};
pub use context::{figma_links, parse_url, Source, CLICKUP_READ_TOOLS, FIGMA_READ_TOOLS};
pub use crew::{of_project as crew_of, Doing, Member};
pub use daemon::{once as tick_once, serve as serve_schedule};
pub use db::{latest_schema, Db};
pub use diff::{parse as parse_diff, patch_for, FileDiff, FileStatus, Hunk, Line, LineKind};
pub use error::{Error, Result};
pub use gates::{all_passed, discover_gates, evidence, run_gates, Gate, GateKind, GateResult};
pub use guardrails::{node_may_continue, run_may_continue, Exceeded, Fallout};
pub use model::TerminalState;
// The Pi runtime (D20): the only runtime there is.
pub use house::{
    read as read_house_rules, read_for as read_house_rules_for, section as house_section, Rules,
};
pub use lsp::{
    language_for, language_id, workspace_root, Client, Diagnostic, Hover, Language, Location, Pool,
    Position, Range,
};
pub use machine::{
    ensure_machine_profile, set_context, set_fallback, set_provider, ContextSource, MachineProfile,
    ModelRegistry, ModelResolution, ProviderState, ProviderStatus, DEFAULT_MACHINE_PROFILE,
};
pub use map::{repo_map, MapEdge, MapNode, MapZone, Owner, RepoMap};
pub use model::{
    Agent, CommentStatus, DiffSide, Event, EventKind, Guardrails, NewAgent, NewComment, NewEvent,
    NewProject, NewReminder, NewRepo, NodeRun, NodeStatus, OnFailure, Project, ProjectKind,
    ProjectRepo, ProjectSource, ProjectStatus, Provider, Reasoning, Recur, Reminder, ReminderKind,
    ReminderStatus, Review, ReviewComment, ReviewStatus, Run, RunStatus, RunTrigger, Team,
    ToolEffect, ToolPolicy, Usage,
};
pub use neighbours::{
    apply_patch_cached, branches, checkout, commit_staged, current_branch, file_sql_available,
    list_tree, push, safe_join, stage, staged_diff, unstage, untracked, worktree_diff, Entry,
    FileSql, Hit, Lease, PlanSummary, Planner, PoolEntry, Question, Slice, Worktrees,
};
pub use onboard::{attach as attach_repo_at, register as register_project, Registered};
pub use paths::{data_dir, default_db_path, ensure_data_dir, machine_profile_path, HOME_ENV};
pub use pi::{
    install_guard, install_guard_at, provider_name as pi_provider, run_pi_turn,
    thinking as pi_thinking, PiDisposition, PiEvent, PiIngested, PiOutcome, PiProcess, PiSeat,
    PiTurn,
};
pub use readiness::{
    apply as apply_fix, apply_at as apply_fix_at, report as readiness_report,
    report_at as readiness_report_at, Action, Check, Fix, Known, Paths, Report, Severity,
};
pub use review::{
    amendment, as_instructions, conductor, deliver as deliver_review, diff_for, for_orchestrator,
    instructions_for, pending as pending_review, responsible, steerable, Audience, Pending,
    Submitted,
};
pub use roles::{
    preset, RolePreset, DEFAULT_LOCAL_MODEL, DEFAULT_ROSTER, ROOT_ROLE, VERIFIER_ROLE,
};
pub use schedule::{act, announcement, claim_due, notify, runnable, tick, Fired, TICK};
pub use speak::{
    queue as queue_message, start_turn, target as speak_target, Reached, Target, Would,
};
pub use store::Store;
pub use supervise::{outcome_status, Dispatched, Orchestration, Orchestrator, Rig, TurnOutcome};
pub use terminal::{Chunk, Listed, Terminals};
pub use today::{
    from_question, from_slice, from_store as today_from_store, rank as rank_today, Item, Urgency,
};
pub use update::{
    apply as apply_update, check as check_update, current_version, is_newer,
    method as install_method, Available, Host, Method, Step,
};
pub use util::{mint_token, normalise_remote, now, rfc3339_in, slugify, zone_matches};
pub use workflow::{run as run_workflow, Progress, Request};

/// The workspace version, compiled in.
///
/// Read from the manifest rather than written out again, because a version constant that
/// has to be kept in sync by hand is a version constant that is wrong.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
