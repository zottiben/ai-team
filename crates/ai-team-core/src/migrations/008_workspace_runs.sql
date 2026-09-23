-- A run belongs to the checkout from which it was started.
--
-- `node_run.worktree_path` cannot answer that question: a team rooted in one checkout
-- leases other worktrees for its makers, and those nodes still belong to the initiating
-- workspace. Recording the run's root is historical context, not copied worktree state.
ALTER TABLE run ADD COLUMN workspace_path TEXT;

-- Existing orchestrated runs already identify their root through the orchestrator node.
-- Other runs with recorded nodes predate checkout-scoped starts and belong to the
-- project's main checkout, including direct turns whose temporary lease was returned.
-- A zero-node run has no evidence of where the old client meant to start it, so leave it
-- unassigned rather than falsely mixing it into main.
UPDATE run
   SET workspace_path = (
       SELECT pr.main_path
         FROM project_repo pr
        WHERE pr.project_id = run.project_id
          AND pr.main_path IS NOT NULL
        ORDER BY pr.ord, pr.id
        LIMIT 1
   )
 WHERE EXISTS (SELECT 1 FROM node_run n WHERE n.run_id = run.id);

UPDATE run
   SET workspace_path = (
       SELECT n.worktree_path
         FROM node_run n
        WHERE n.run_id = run.id
          AND n.role = 'orchestrator'
          AND n.worktree_path IS NOT NULL
        ORDER BY n.id
        LIMIT 1
   )
 WHERE EXISTS (
       SELECT 1
         FROM node_run n
        WHERE n.run_id = run.id
          AND n.role = 'orchestrator'
          AND n.worktree_path IS NOT NULL
   );

CREATE INDEX run_workspace ON run(project_id, workspace_path, id DESC);

DROP VIEW v_runs;
CREATE VIEW v_runs AS
SELECT
    r.id,
    p.slug AS project,
    t.slug AS team,
    r.status,
    r.trigger,
    r.plan_slug,
    r.workspace_path,
    (SELECT COUNT(*) FROM node_run n WHERE n.run_id = r.id)                      AS nodes,
    (SELECT COUNT(*) FROM node_run n WHERE n.run_id = r.id AND n.status = 'done') AS nodes_done,
    (SELECT COALESCE(SUM(n.tokens_in + n.tokens_out), 0) FROM node_run n WHERE n.run_id = r.id) AS tokens,
    (SELECT COALESCE(SUM(n.tokens_cache_read), 0) FROM node_run n WHERE n.run_id = r.id)        AS tokens_cached,
    r.blocked_reason,
    r.started_at,
    r.ended_at,
    r.prompt
FROM run r
JOIN project p ON p.id = r.project_id
LEFT JOIN team t ON t.id = r.team_id;
