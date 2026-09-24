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
mod delivery;
mod dialog;
mod diff;
mod error;
mod gates;
mod guardrails;
mod house;
mod launch_path;
mod layout;
mod lsp;
mod machine;
mod map;
mod model;
mod neighbours;
mod onboard;
mod paths;
mod pi;
mod readiness;
mod restack;
mod review;
mod roles;
mod schedule;
mod seats;
mod secrets;
mod skills;
mod speak;
mod stack;
mod store;
mod supervise;
mod tasks;
mod terminal;
mod today;
mod update;
mod util;
mod workflow;
mod workspace;

pub use analytics::{rollup, rollup_workspace, By, Row};
pub use context::{figma_links, parse_url, Source, CLICKUP_READ_TOOLS, FIGMA_READ_TOOLS};
pub use crew::{of_project as crew_of, of_workspace as crew_of_workspace, Doing, Member};
pub use daemon::{once as tick_once, serve as serve_schedule};
pub use db::{latest_schema, Db};
pub use delivery::{approve_delivery, delivery_status};
pub use dialog::alert;
pub use diff::{parse as parse_diff, patch_for, FileDiff, FileStatus, Hunk, Line, LineKind};
pub use error::{Error, Result};
pub use gates::{all_passed, discover_gates, evidence, run_gates, Gate, GateKind, GateResult};
pub use guardrails::{node_may_continue, run_may_continue, Exceeded, Fallout};
pub use model::TerminalState;
// The Pi runtime (D20): the only runtime there is.
pub use house::{
    read as read_house_rules, read_for as read_house_rules_for, section as house_section, Rules,
};
pub use launch_path::adopt_login_path;
pub use layout::{
    place as place_worktrees, Kind as WorktreeKind, Placed as PlacedWorktree, PrFacts,
};
pub use lsp::{
    language_for, language_id, workspace_root, Client, Diagnostic, Hover, Language, Location, Pool,
    Position, Range,
};
pub use machine::{
    ensure_machine_profile, set_context, set_fallback, set_provider, sign_in_command,
    ContextSource, MachineProfile, ModelChoice, ModelRegistry, ModelResolution, ProviderState,
    ProviderStatus, RoleModelDefault, Stranded, DEFAULT_MACHINE_PROFILE,
};
pub use map::{repo_map, MapEdge, MapNode, MapZone, Owner, RepoMap};
pub use model::{
    Agent, CommentStatus, DeliveryAction, DeliveryPolicy, DeliverySettings, DiffSide, Event,
    EventKind, Guardrails, NewAgent, NewComment, NewEvent, NewNotification, NewProject,
    NewReminder, NewRepo, NodeRun, NodeStatus, Notification, OnFailure, Project, ProjectKind,
    ProjectRepo, ProjectSource, ProjectStatus, Provider, Reasoning, Recur, Reminder, ReminderKind,
    ReminderStatus, RemoteDeliveryStatus, Review, ReviewComment, ReviewStatus, Run, RunOrigin,
    RunStatus, RunTrigger, Team, ToolEffect, ToolPolicy, Usage,
};
pub use neighbours::{
    apply_patch_cached, branches, checkout, commit_staged, current_branch, file_sql_available,
    list_tree, push, safe_join, same_worktree, stage, staged_diff, unstage, untracked,
    worktree_diff, Entry, FileSql, Hit, Lease, PlanLogEntry, PlanSummary, Planner, PoolEntry,
    Process, Question, Slice, Worktrees,
};
pub use onboard::{
    attach as attach_repo_at, browse as browse_dir, register as register_project,
    zones_for as suggested_zones, Candidate, Listing, Registered,
};
pub use paths::{
    data_dir, default_db_path, ensure_data_dir, expand_user, home_dir, machine_profile_path,
    HOME_ENV,
};
pub use pi::{
    install_guard, install_guard_at, provider_name as pi_provider, run_pi_turn,
    thinking as pi_thinking, write_context_oauth_config, PiDisposition, PiEvent, PiIngested,
    PiOutcome, PiPlanAccess, PiProcess, PiSeat, PiTurn,
};
pub use readiness::{
    apply as apply_fix, apply_at as apply_fix_at, report as readiness_report,
    report_at as readiness_report_at, report_at_with_credentials as readiness_report_at_with,
    report_with_credentials as readiness_report_with, Action, Check, Fix, Known, Paths, Report,
    Severity,
};
pub use review::{
    amendment, as_instructions, conductor, deliver as deliver_review, diff_against, diff_for,
    follow_up_ready, follow_up_target, for_orchestrator, instructions_for,
    pending as pending_review, responsible, steerable, Audience, FollowUp, Pending, Submitted,
};
pub use roles::{
    preset, RolePreset, DEFAULT_LOCAL_MODEL, DEFAULT_ROSTER, ROOT_ROLE, VERIFIER_ROLE,
};
pub use schedule::{act, announcement, claim_due, notify, runnable, tick, Fired, TICK};
pub use seats::{describe as describe_stranded, reseat_stranded, Moved, Reseated, Survey};
pub use secrets::{
    clear_token, forget_oauth, has_oauth, has_token, held as token_held, set_token,
    token as context_token, token_env, CredentialStore, Held,
};
pub use speak::{
    queue as queue_message, start_turn, start_turn_in, target as speak_target,
    target_in as speak_target_in, Reached, Target, Would,
};
pub use stack::parent as stacked_on;
pub use store::{Abandoned, Store, INTERRUPTED_REASON};
pub use supervise::{outcome_status, Dispatched, Orchestration, Orchestrator, Rig, TurnOutcome};
pub use terminal::{Chunk, Listed, Terminals};
pub use today::{
    checkouts as today_checkouts, from_plans as today_from_plans, from_question, from_slice,
    from_store as today_from_store, rank as rank_today, Item, Urgency,
};
pub use update::{
    apply as apply_update, check as check_update, current_version, is_newer,
    method as install_method, repair as repair_app, replaced_app, Available, Host, Method, Step,
};
pub use util::{
    mint_token, normalise_remote, now, process_is_alive, rfc3339_in, slugify, zone_matches,
};
pub use workflow::{
    approve_at as approve_run_at, claim_plan_approval, claim_plan_approval_with_direction,
    claim_session_reset, continue_approved_at as continue_approved_run_at, follow_up_at,
    open_follow_up, reset_claimed_session_at as reset_claimed_session,
    resume_interrupted_node_at as resume_interrupted_node, run as run_workflow,
    settle_abandoned_runs, Branching, Progress, Request, Resumed, PLAN_APPROVAL_PREPARING_REASON,
    PLAN_APPROVAL_REASON,
};

/// The workspace version, compiled in.
///
/// Read from the manifest rather than written out again, because a version constant that
/// has to be kept in sync by hand is a version constant that is wrong.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
