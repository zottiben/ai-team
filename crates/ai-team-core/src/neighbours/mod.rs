//! The tools ai-team borrows rather than absorbs (D4).
//!
//! ai-planner owns the work graph, ai-worktree owns worktree isolation, and file-sql owns
//! code search. All three are used over their own command line, the same one a human uses,
//! so all three keep working standalone and none has a second implementation living here.

mod filesql;
pub(crate) mod git;
mod planner;
mod worktree;

pub use filesql::{
    available as file_sql_available, list as list_tree, safe_join, Entry, FileSql, Hit,
};
pub use planner::{PlanSummary, Planner, Question, Slice};
pub use worktree::{Lease, PoolEntry, Worktrees};
