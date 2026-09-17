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

mod db;
mod error;
mod model;
mod paths;
mod roles;
mod store;
mod util;

pub use db::Db;
pub use error::{Error, Result};
pub use model::{
    Agent, CommentStatus, DiffSide, Event, EventKind, Guardrails, NewAgent, NewComment, NewEvent,
    NewProject, NewReminder, NewRepo, NodeRun, NodeStatus, OnFailure, Project, ProjectKind,
    ProjectRepo, ProjectSource, ProjectStatus, Provider, Reasoning, Recur, Reminder, ReminderKind,
    ReminderStatus, Review, ReviewComment, ReviewStatus, Run, RunStatus, RunTrigger, Team,
    ToolEffect, Usage,
};
pub use paths::{data_dir, default_db_path, ensure_data_dir, machine_profile_path, HOME_ENV};
pub use roles::{preset, RolePreset, DEFAULT_LOCAL_MODEL, DEFAULT_ROSTER};
pub use store::Store;
pub use util::{normalise_remote, now, slugify, zone_matches};

/// The workspace version, compiled in.
///
/// Read from the manifest rather than written out again, because a version constant that
/// has to be kept in sync by hand is a version constant that is wrong.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
