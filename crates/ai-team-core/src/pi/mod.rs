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

mod event;
mod guard;
mod ingest;
mod process;

pub use event::Disposition as PiDisposition;
pub use event::PiEvent;
pub use guard::{install as install_guard, install_at as install_guard_at};
pub use ingest::PiIngested;
pub use process::{PiProcess, PiTurn, TurnOutcome as PiOutcome};
