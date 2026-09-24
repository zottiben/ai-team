//! What a run and its nodes are allowed to spend, and what happens when they stop.
//!
//! Every limit here is read off the **run**, never back through the team (D2). A run
//! started last night under a 40-turn cap did not become a 200-turn run because somebody
//! raised the team's cap this morning.
//!
//! The checks are deliberately boring arithmetic in one place. Spread across call sites
//! they drift, and a budget that is enforced in three places and forgotten in a fourth is
//! the one that costs somebody a rate limit at 2am.

use crate::error::Result;
use crate::model::{NodeRun, OnFailure, Run};
use crate::store::Store;
use crate::util::now;

/// Why work stopped, in words a human reads on a blocked row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exceeded {
    pub reason: String,
    /// True when the whole run is finished, not just this node. A run-wide budget stops
    /// its siblings too; a node's own cap does not.
    pub run_wide: bool,
}

impl Exceeded {
    fn node(reason: impl Into<String>) -> Exceeded {
        Exceeded {
            reason: reason.into(),
            run_wide: false,
        }
    }

    fn run(reason: impl Into<String>) -> Exceeded {
        Exceeded {
            reason: reason.into(),
            run_wide: true,
        }
    }
}

/// How long a run has been going, in seconds, or 0 before it starts.
fn elapsed(started_at: Option<&str>) -> i64 {
    let Some(started) = started_at else {
        return 0;
    };
    let parse = |text: &str| {
        time::PrimitiveDateTime::parse(
            text,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .ok()
        .map(|at| at.assume_utc().unix_timestamp())
    };
    match (parse(started), parse(&now())) {
        (Some(from), Some(to)) => (to - from).max(0),
        _ => 0,
    }
}

/// May this run start another node?
///
/// Checked before a node is dispatched rather than only after it finishes: the point of
/// a budget is to not spend the next turn, and noticing afterwards is an audit trail
/// rather than a limit.
pub fn run_may_continue(store: &Store, run_id: i64) -> Result<Option<Exceeded>> {
    let run = store.run(run_id)?;
    let usage = store.run_usage(run_id)?;

    if let Some(budget) = run.budget_tokens {
        let spent = usage.billable();
        if spent >= budget {
            return Ok(Some(Exceeded::run(format!(
                "the run's token budget is spent: {spent} of {budget} billable tokens"
            ))));
        }
    }
    if let Some(budget) = run.budget_seconds {
        let spent = elapsed(run.started_at.as_deref());
        if spent >= budget {
            return Ok(Some(Exceeded::run(format!(
                "the run's time budget is spent: {spent}s of {budget}s"
            ))));
        }
    }
    Ok(None)
}

/// May this node take another turn?
///
/// Both the node's own caps and the run-wide ones, because a node that is within its own
/// budget still cannot spend a run that has none left.
pub fn node_may_continue(store: &Store, node_run_id: i64) -> Result<Option<Exceeded>> {
    let node = store.node_run(node_run_id)?;
    let run = store.run(node.run_id)?;

    if let Some(exceeded) = run_may_continue(store, node.run_id)? {
        return Ok(Some(exceeded));
    }
    Ok(node_caps(&run, &node))
}

/// The node's own limits, separated so they can be checked against a node that is still
/// in flight without a second round trip for the run.
fn node_caps(run: &Run, node: &NodeRun) -> Option<Exceeded> {
    if let Some(budget) = run.budget_tokens_node {
        let spent = node.usage.billable();
        if spent >= budget {
            return Some(Exceeded::node(format!(
                "this node's token budget is spent: {spent} of {budget} billable tokens"
            )));
        }
    }
    if let Some(budget) = run.budget_seconds_node {
        let spent = elapsed(node.started_at.as_deref());
        if spent >= budget {
            return Some(Exceeded::node(format!(
                "this node's time budget is spent: {spent}s of {budget}s"
            )));
        }
    }
    if let Some(cap) = run.max_turns_node {
        if node.turns >= cap {
            return Some(Exceeded::node(format!(
                "this node has taken {} turns, its cap is {cap}",
                node.turns
            )));
        }
    }
    None
}

/// What to do about a node that failed and has no repairs left.
///
/// The policy is the team's, snapshotted onto the run. All three leave the siblings
/// alone: a failing node fails its own branch, and a plan is not poisoned because one
/// slice did not work out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallout {
    /// Park it and wait for a person. The work stays claimed, because it is still theirs.
    Escalate,
    /// Give up on this slice and let the rest of the run finish.
    AbortBranch,
}

impl Fallout {
    pub fn of(policy: OnFailure) -> Fallout {
        match policy {
            // Retry is spent by the time this is asked: the repair loop is the retrying,
            // and reaching here means it ran out. Falling back to abort keeps the run
            // moving rather than parking on something nobody asked to be asked about.
            OnFailure::Retry | OnFailure::AbortBranch => Fallout::AbortBranch,
            OnFailure::Escalate => Fallout::Escalate,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Fallout::Escalate => "escalate",
            Fallout::AbortBranch => "abort_branch",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewProject, RunTrigger, Usage};

    fn seeded() -> (Store, i64, i64) {
        let mut store = Store::memory().unwrap();
        let project = store
            .create_project(NewProject {
                name: "Widget".into(),
                ..Default::default()
            })
            .unwrap();
        let team = store
            .seed_default_team(project.id, &crate::RoleModelDefault::local_floor())
            .unwrap();
        (store, project.id, team.id)
    }

    fn backend(store: &Store, team: i64) -> i64 {
        store
            .agents(team)
            .unwrap()
            .into_iter()
            .find(|agent| agent.role == "backend")
            .unwrap()
            .id
    }

    fn set(store: &mut Store, run_id: i64, column: &str, value: i64) {
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    &format!("UPDATE run SET {column} = ?2 WHERE id = ?1"),
                    rusqlite::params![run_id, value],
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_run_budget_stops_the_whole_run_and_a_node_cap_does_not() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        set(&mut store, run.id, "budget_tokens", 5_000);
        set(&mut store, run.id, "budget_tokens_node", 1_000);

        let node = store
            .dispatch(
                run.id,
                backend(&store, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        assert!(node_may_continue(&store, node.id).unwrap().is_none());

        // Past the node's budget but not the run's: this node stops, the run does not.
        store
            .record_usage(
                node.id,
                Usage {
                    tokens_in: 1_000,
                    tokens_out: 0,
                    cache_read: 500_000,
                    cache_write: 0,
                },
                1,
            )
            .unwrap();
        let stopped = node_may_continue(&store, node.id).unwrap().unwrap();
        assert!(!stopped.run_wide, "a node's own cap is its own");
        assert!(stopped.reason.contains("this node's token budget"));
        assert!(
            run_may_continue(&store, run.id).unwrap().is_none(),
            "the run still has budget for its other nodes"
        );

        // Past the run's budget: everything stops.
        store
            .record_usage(
                node.id,
                Usage {
                    tokens_in: 4_000,
                    tokens_out: 0,
                    cache_read: 0,
                    cache_write: 0,
                },
                1,
            )
            .unwrap();
        let stopped = run_may_continue(&store, run.id).unwrap().unwrap();
        assert!(stopped.run_wide);
        assert!(stopped.reason.contains("the run's token budget"));
    }

    #[test]
    fn the_turn_cap_is_counted_in_turns_not_attempts() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        set(&mut store, run.id, "max_turns_node", 3);
        let node = store
            .dispatch(
                run.id,
                backend(&store, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();

        store.record_usage(node.id, Usage::default(), 2).unwrap();
        assert!(node_may_continue(&store, node.id).unwrap().is_none());

        store.record_usage(node.id, Usage::default(), 1).unwrap();
        let stopped = node_may_continue(&store, node.id).unwrap().unwrap();
        assert!(stopped.reason.contains("3 turns"), "{}", stopped.reason);
        assert!(!stopped.run_wide);
    }

    #[test]
    fn a_run_snapshots_the_node_caps_too() {
        // The whole point of snapshotting: raising the team's cap must not raise it for
        // a run that is already going.
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        assert_eq!(run.max_turns_node, Some(40));
        assert_eq!(run.budget_tokens_node, Some(400_000));
        assert_eq!(run.on_failure, OnFailure::Retry);

        let mut guardrails = store.team(team).unwrap().guardrails;
        guardrails.max_turns_node = Some(9_999);
        guardrails.on_failure = OnFailure::Escalate;
        store
            .update_team(team, "Widget team", "", guardrails)
            .unwrap();

        let unchanged = store.run(run.id).unwrap();
        assert_eq!(unchanged.max_turns_node, Some(40));
        assert_eq!(unchanged.on_failure, OnFailure::Retry);
    }

    #[test]
    fn every_failure_policy_leaves_the_siblings_alone() {
        // The one property all three share, and the one this slice exists to guarantee:
        // a node failing is its branch's problem, not the run's.
        assert_eq!(Fallout::of(OnFailure::Escalate), Fallout::Escalate);
        assert_eq!(Fallout::of(OnFailure::AbortBranch), Fallout::AbortBranch);
        // Retry has already been spent by the repair loop by the time this is asked.
        assert_eq!(Fallout::of(OnFailure::Retry), Fallout::AbortBranch);
    }

    #[test]
    fn a_budget_of_none_is_unlimited_rather_than_zero() {
        let (mut store, project, team) = seeded();
        let run = store
            .create_run(project, "ship it", RunTrigger::Manual)
            .unwrap();
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE run SET budget_tokens = NULL, budget_tokens_node = NULL,
                        budget_seconds = NULL, max_turns_node = NULL WHERE id = ?1",
                    rusqlite::params![run.id],
                )?;
                Ok(())
            })
            .unwrap();
        let node = store
            .dispatch(
                run.id,
                backend(&store, team),
                Some("PR1"),
                &crate::ModelRegistry::local_only(),
            )
            .unwrap();
        store
            .record_usage(
                node.id,
                Usage {
                    tokens_in: 10_000_000,
                    tokens_out: 0,
                    cache_read: 0,
                    cache_write: 0,
                },
                500,
            )
            .unwrap();
        assert!(node_may_continue(&store, node.id).unwrap().is_none());
    }
}
