//! The tools ai-team borrows rather than absorbs (D4).
//!
//! ai-planner owns the work graph and ai-worktree owns worktree isolation. Both are used
//! over their own command line, the same one a human uses, so both keep working standalone
//! and neither has a second implementation living in here.

pub(crate) mod git;
mod planner;
mod worktree;

pub use planner::{Planner, Slice};
pub use worktree::{Lease, PoolEntry, Worktrees};
