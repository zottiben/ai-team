//! The Pi runtime: one process per leased worktree (D20).
//!
//! Pi is a coding agent that ai-team drives as a child process rather than a server. That
//! is the whole shape of this module, and it is why it is so much smaller than the eve
//! one it replaces: there is no project to generate, no npm install, no build, no port,
//! no token, and no HTTP client. A turn is `pi --mode json -p <prompt>` with its working
//! directory on the lease, and its stdout is the stream.
//!
//! What ai-team still owns is unchanged: which seat runs, on what model, in which
//! worktree, under what budget, and what is written to the database.

mod chat_team;
mod event;
mod guard;
mod ingest;
mod instructions;
mod live;
mod process;
mod seat;
mod turn;

pub(crate) use chat_team::{
    planning_turn as chat_team_planning_turn, worker_turn as chat_team_worker_turn,
};
pub use event::Disposition as PiDisposition;
pub use event::PiEvent;
pub use guard::{install as install_guard, install_at as install_guard_at};
pub use ingest::PiIngested;
pub(crate) use process::strip_metered_env;
pub use process::{PiProcess, PiTurn, TurnOutcome as PiOutcome};
pub(crate) use seat::conversation_turn;
pub use seat::{provider_name, thinking, write_context_oauth_config, Seat as PiSeat};
pub use turn::run as run_pi_turn;
pub(crate) use turn::run_until;
