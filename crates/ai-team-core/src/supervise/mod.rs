//! Running eve, and driving it.
//!
//! Layered on purpose: `http` knows about sockets and framing, `client` knows about
//! `/eve/v1`, `process` knows about children and ports, and `supervisor` knows the
//! order they go in. Only the top layer touches the store.

mod client;
mod http;
mod process;
mod supervisor;

pub use client::{approvals_in, AgentInfo, Approval, ApprovalOption, EveClient};
pub use process::{free_port, mint_token, EveEnv, EveProcess, ProgressLine};
pub use supervisor::{
    drive_turn, reattach, record_build_progress, run_turn, BuildPhase, BuildProgress, Supervisor,
    TurnOutcome,
};

pub use http::Flow;
