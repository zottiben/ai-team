-- A pull request is built as its tasks (PW4). Each task's turns are node runs carrying
-- the task's key from the slice's scope - a reference into the plan, like `slice_key`,
-- never a copy of the task (D4). NULL is a slice built as one piece of work, which is
-- what every slice was before and what a slice with no task lines still is.
ALTER TABLE node_run ADD COLUMN task_key TEXT;

-- v_node_runs gains the task beside its slice, so TablePlus says which step a row was.
DROP VIEW v_node_runs;

CREATE VIEW v_node_runs AS
SELECT
    n.id,
    p.slug AS project,
    n.run_id,
    n.role,
    n.provider,
    n.model,
    n.status,
    n.attempt,
    n.slice_key,
    n.task_key,
    n.branch,
    n.worktree_path,
    n.eve_port,
    n.stream_cursor,
    (SELECT COUNT(*) FROM event e WHERE e.node_run_id = n.id) AS events,
    n.turns,
    n.tokens_in,
    n.tokens_out,
    n.tokens_cache_read,
    n.tokens_cache_write,
    n.blocked_reason,
    n.started_at,
    n.ended_at
FROM node_run n
JOIN run r ON r.id = n.run_id
JOIN project p ON p.id = r.project_id;
