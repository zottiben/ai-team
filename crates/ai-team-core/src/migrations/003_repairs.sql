-- The repair budget belongs to the run, not to the team it came from (M2-S9).
--
-- `run` already snapshots parallel_width and the token/second budgets at dispatch, for
-- the reason stated there: what a run was allowed to spend is a fact about that run, and
-- reading it back through `team` months later answers a different question. How many
-- times a failing node may be repaired is the same kind of fact, and it was the one
-- guardrail still being read live.
ALTER TABLE run ADD COLUMN max_repairs INTEGER NOT NULL DEFAULT 2;
