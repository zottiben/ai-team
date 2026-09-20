//! What a turn amounted to, and what that makes of the node that took it.
//!
//! Deliberately not in `pi`. A turn outcome describes a turn - what it recorded, what it
//! spent, how it ended - and none of that is a fact about which runtime took it. Keeping
//! it here is what let the whole dispatch loop stay unchanged when the runtime was
//! replaced underneath it (D20).

use crate::model::{NodeStatus, TerminalState, Usage};

/// What one turn amounted to.
#[derive(Debug, Clone, Default)]
pub struct TurnOutcome {
    pub recorded: usize,
    pub duplicates: usize,
    pub usage: Usage,
    pub steps: i64,
    /// How the turn ended, or `None` while it is in flight.
    pub terminal: Option<TerminalState>,
}

/// What a finished turn makes of its node.
///
/// A turn with no terminal state failed: the stream is the record, and one that stopped
/// without saying so is a child that died mid-turn. Reading it the other way is how a
/// truncated turn gets recorded as a clean one.
pub fn outcome_status(outcome: &TurnOutcome) -> NodeStatus {
    match outcome.terminal {
        Some(TerminalState::Completed) => NodeStatus::Done,
        Some(TerminalState::Cancelled) => NodeStatus::Cancelled,
        Some(TerminalState::Failed) | None => NodeStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_that_never_said_how_it_ended_failed() {
        // The stream is the record. A turn that stopped without a terminal event is a
        // child that died, and reading silence as success hides exactly that.
        assert_eq!(outcome_status(&TurnOutcome::default()), NodeStatus::Failed);
    }

    #[test]
    fn each_terminal_state_lands_where_it_should() {
        for (terminal, status) in [
            (TerminalState::Completed, NodeStatus::Done),
            (TerminalState::Cancelled, NodeStatus::Cancelled),
            (TerminalState::Failed, NodeStatus::Failed),
        ] {
            let outcome = TurnOutcome {
                terminal: Some(terminal),
                ..TurnOutcome::default()
            };
            assert_eq!(outcome_status(&outcome), status, "{terminal:?}");
        }
    }
}
