//! Driving a run: what to dispatch, to whom, and what to make of the result.
//!
//! Everything about *how* a turn is taken lives in `pi` (D20). What is left here is the
//! part that was never about a runtime - routing a slice to the seat whose zone owns it,
//! leasing a worktree for it, running the project's own gates, asking the verifier, and
//! deciding what the answer makes of the node.

mod orchestrate;
mod outcome;

pub use orchestrate::{Dispatched, Orchestration, Orchestrator, Rig};
pub(crate) use outcome::quota_exhaustion;
pub use outcome::{outcome_status, TurnOutcome};
