-- A run starts its checkout on a fresh branch off the default branch, and two facts about
-- that belong to the run rather than to the team.
--
-- `on_default_branch` is the operator saying, for this run only, that work may land on
-- main/master itself. Snapshotted like the guardrails: an approval continued later, in
-- another process, must be held to what the run was started with.
--
-- `supervisor_pid` is the process driving the run. Switching a checkout's branch under a
-- run that is still planning in it would pull the floor out from under that run, so a
-- second run is refused while the first is alive - and only while it is alive, because a
-- crashed run never gets to mark itself finished and would otherwise hold the checkout
-- forever.
ALTER TABLE run ADD COLUMN on_default_branch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE run ADD COLUMN supervisor_pid INTEGER;
