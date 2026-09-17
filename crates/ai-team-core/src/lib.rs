//! The org graph, the store and the run state behind ai-team.
//!
//! This crate is where the things we need to see live: team and agent rows, routing,
//! budgets, run and step state (pillar 1). Everything above it - the CLI, the local
//! server, the desktop shell - is a presentation of what is in here, and none of them
//! keeps state of its own.
//!
//! `M0-S1` establishes the crate and the paths every surface agrees on. The schema and
//! the store arrive in `M0-S2`.

mod error;
mod paths;

pub use error::{Error, Result};
pub use paths::{data_dir, default_db_path, ensure_data_dir, machine_profile_path, HOME_ENV};

/// The workspace version, compiled in.
///
/// Read from the manifest rather than written out again, because a version constant
/// that has to be kept in sync by hand is a version constant that is wrong.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
