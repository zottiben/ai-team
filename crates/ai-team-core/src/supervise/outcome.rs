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
    /// The final assistant/provider diagnostic. Kept separately from the append-only
    /// event stream so supervision can classify account availability without parsing
    /// ai-team's own Note rows back out of the database.
    pub provider_message: Option<String>,
    /// How the turn ended, or `None` while it is in flight.
    pub terminal: Option<TerminalState>,
}

/// A provider's subscription allowance ended, rather than the implementation failing.
///
/// Keep this intentionally narrower than generic 429/network matching. D19 permits an
/// account failover for a known quota, not silently moving work after a transient fault.
pub(crate) fn quota_exhaustion(outcome: &TurnOutcome) -> Option<String> {
    let message = outcome.provider_message.as_deref()?.trim();
    let lower = message.to_lowercase();
    let known = [
        "you've hit your session limit",
        "you’ve hit your session limit",
        "you have hit your session limit",
        "you've hit your usage limit",
        "you’ve hit your usage limit",
        "usage limit has been reached",
        "insufficient_quota",
    ];
    if !known.iter().any(|needle| lower.contains(needle)) {
        return None;
    }

    provider_diagnostic(outcome)
}

/// The provider's useful terminal diagnostic, without adapter echo.
///
/// Claude's Pi adapter currently emits some error text twice. Preserve the evidence in
/// the raw event payload, but do not make the board and notification repeat themselves.
///
/// Pi prints a fatal error as `Error: ...`, after whatever its extensions said while it
/// started. When it did, that line leads: a reason is read by its first line, and an
/// extension's warning there hid why the turn failed.
pub(crate) fn provider_diagnostic(outcome: &TurnOutcome) -> Option<String> {
    let message = outcome.provider_message.as_deref()?.trim();
    let mut unique = Vec::new();
    for line in message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !unique.iter().any(|seen| seen == &line) {
            unique.push(line);
        }
    }
    if let Some(error) = unique.iter().position(|line| line.starts_with("Error:")) {
        unique.drain(..error);
    }
    if unique.is_empty() {
        return None;
    }
    let message = unique.join("\n");
    let lower = message.to_lowercase();
    if lower.contains("not logged in") && lower.contains("/login") {
        return Some(
            "The selected provider is not signed in. Open Settings and sign in before retrying this slice."
                .into(),
        );
    }
    Some(message)
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
    fn a_subscription_limit_is_account_availability_not_a_successful_turn() {
        let outcome = TurnOutcome {
            terminal: Some(TerminalState::Completed),
            provider_message: Some(
                "You've hit your session limit · resets 10:50pm (Australia/Adelaide)\n\
                 You've hit your session limit · resets 10:50pm (Australia/Adelaide)"
                    .into(),
            ),
            ..TurnOutcome::default()
        };

        let exhausted = quota_exhaustion(&outcome).expect("recognized quota");
        assert_eq!(
            exhausted,
            "You've hit your session limit · resets 10:50pm (Australia/Adelaide)"
        );
    }

    #[test]
    fn an_ordinary_model_or_network_failure_is_not_silently_rerouted() {
        for message in [
            "connection reset by peer",
            "tool call failed",
            "429 requests are arriving too quickly",
        ] {
            let outcome = TurnOutcome {
                provider_message: Some(message.into()),
                ..TurnOutcome::default()
            };
            assert_eq!(quota_exhaustion(&outcome), None, "{message}");
        }
    }

    #[test]
    fn repeated_provider_errors_are_one_operator_diagnostic() {
        let outcome = TurnOutcome {
            provider_message: Some(
                "Not logged in · Please run /login\nNot logged in · Please run /login".into(),
            ),
            ..TurnOutcome::default()
        };
        assert_eq!(
            provider_diagnostic(&outcome).as_deref(),
            Some(
                "The selected provider is not signed in. Open Settings and sign in before retrying this slice."
            )
        );
    }

    #[test]
    fn pis_own_error_leads_rather_than_an_extensions_warning_before_it() {
        // Extensions print to stderr as Pi starts, and a reason read by its first line - a
        // table cell, a notification - showed the warning and hid why the turn failed.
        let outcome = TurnOutcome {
            provider_message: Some(
                "[pi-web-access] Dynamic tool activation requires Pi 0.86.1 or newer.\n\
                 Error: Unknown provider \"llama.cpp\". Use --list-models to see available \
                 providers/models."
                    .into(),
            ),
            ..TurnOutcome::default()
        };
        assert_eq!(
            provider_diagnostic(&outcome).as_deref(),
            Some(
                "Error: Unknown provider \"llama.cpp\". Use --list-models to see available \
                 providers/models."
            )
        );

        // With no error line, everything it said is the diagnostic.
        let outcome = TurnOutcome {
            provider_message: Some("connection reset by peer".into()),
            ..TurnOutcome::default()
        };
        assert_eq!(
            provider_diagnostic(&outcome).as_deref(),
            Some("connection reset by peer")
        );
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
