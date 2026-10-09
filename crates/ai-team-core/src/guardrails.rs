//! Failure policy for work that has exhausted its repair allowance.
//!
//! Policy is snapshotted onto the run, never reread from an edited team. Token usage,
//! elapsed time and turn counts are evidence, not execution limits (D33, D35). Retired
//! budget columns remain historical evidence; no execution path enforces them.

use crate::model::OnFailure;

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
    use crate::{NewProject, RunTrigger, Store};

    #[test]
    fn retired_team_caps_are_not_copied_and_run_policy_is_still_snapshotted() {
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
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE team SET budget_tokens_run=2000000, budget_tokens_node=400000,
                budget_seconds_run=3600, budget_seconds_node=900, max_turns_node=40 WHERE id=?1",
                    [team.id],
                )?;
                Ok(())
            })
            .unwrap();
        let run = store
            .create_run(project.id, "ship it", RunTrigger::Manual)
            .unwrap();
        assert_eq!(run.budget_tokens, None);
        assert_eq!(run.budget_tokens_node, None);
        assert_eq!(run.budget_seconds, None);
        assert_eq!(run.budget_seconds_node, None);
        assert_eq!(run.max_turns_node, None);
        assert_eq!(run.max_repairs, 2);
        assert_eq!(run.on_failure, OnFailure::Retry);

        // Retained budget evidence is not cleared when a team is edited, nor presented
        // as active configuration. New policy still cannot rewrite a run's repair limit.
        store
            .db_mut()
            .write(|tx| {
                tx.execute(
                    "UPDATE run SET budget_tokens=2000000, budget_tokens_node=400000,
                budget_seconds=3600, budget_seconds_node=900, max_turns_node=40 WHERE id=?1",
                    [run.id],
                )?;
                Ok(())
            })
            .unwrap();
        let mut guardrails = store.team(team.id).unwrap().guardrails;
        guardrails.max_repairs = 10;
        guardrails.on_failure = OnFailure::Escalate;
        store
            .update_team(team.id, "Widget team", "", guardrails)
            .unwrap();
        let unchanged = store.run(run.id).unwrap();
        assert_eq!(unchanged.budget_tokens, Some(2_000_000));
        assert_eq!(unchanged.budget_tokens_node, Some(400_000));
        assert_eq!(unchanged.budget_seconds, Some(3_600));
        assert_eq!(unchanged.budget_seconds_node, Some(900));
        assert_eq!(unchanged.max_turns_node, Some(40));
        assert_eq!(unchanged.max_repairs, 2);
        assert_eq!(unchanged.on_failure, OnFailure::Retry);
        let retired: (i64, i64, i64, i64, i64) = store
            .db()
            .conn()
            .query_row(
                "SELECT budget_tokens_run, budget_tokens_node, budget_seconds_run,
                budget_seconds_node, max_turns_node FROM team WHERE id=?1",
                [team.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(retired, (2_000_000, 400_000, 3_600, 900, 40));
        let policy = serde_json::to_value(guardrails).unwrap();
        assert_eq!(policy.as_object().unwrap().len(), 3);
        assert!(policy.get("budget_tokens_run").is_none());
    }

    #[test]
    fn every_failure_policy_leaves_the_siblings_alone() {
        assert_eq!(Fallout::of(OnFailure::Escalate), Fallout::Escalate);
        assert_eq!(Fallout::of(OnFailure::AbortBranch), Fallout::AbortBranch);
        // Retry has already been spent by the repair loop by the time this is asked.
        assert_eq!(Fallout::of(OnFailure::Retry), Fallout::AbortBranch);
    }
}
