-- The rest of the guardrails, snapshotted onto the run (M2-S10).
--
-- `run` already carries parallel_width, the run-wide budgets and max_repairs, for the
-- reason the table states: what a run was allowed to spend is a fact about that run, and
-- reading it back through `team` months later answers a different question.
--
-- These four were still only on `team`, which meant a node's caps and a team's failure
-- policy could change underneath a run that was already going. A run started last night
-- under a 40-turn cap did not become a 200-turn run because somebody raised it this
-- morning.
ALTER TABLE run ADD COLUMN budget_tokens_node INTEGER;
ALTER TABLE run ADD COLUMN budget_seconds_node INTEGER;
ALTER TABLE run ADD COLUMN max_turns_node INTEGER;
ALTER TABLE run ADD COLUMN on_failure TEXT NOT NULL DEFAULT 'retry'
    CHECK (on_failure IN ('retry','escalate','abort_branch'));
