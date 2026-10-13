-- Pi's thinking range is wider than the four levels ai-team shipped with: a seat can be
-- asked for `off`, `minimal`, `xhigh` or `max` too. The stored vocabulary stays canonical
-- (`none` is how this schema has always spelled "off"), so only the CHECKs widen; no row
-- is rewritten and no column changes name or type.
--
-- SQLite cannot alter a CHECK in place, so each table is rebuilt by the documented
-- procedure. The runner takes care of the parts that cannot live in a script: foreign
-- keys are off for this migration only, `legacy_alter_table` keeps RENAME from reparsing
-- objects that reference a table mid-rebuild, and `foreign_key_check` must pass before
-- the transaction commits. Every column, row, id, index, trigger and view below is the
-- one that was there before - this file is a copy with four words added to three CHECKs.

-- Views are dropped first and recreated at the end: a view over a table being rebuilt is
-- a view that cannot be parsed while the table is gone.
DROP VIEW v_projects;
DROP VIEW v_agents;
DROP VIEW v_events;
DROP VIEW v_runs;
DROP VIEW v_node_runs;

CREATE TABLE agent_rebuilt (
    id          INTEGER PRIMARY KEY,
    team_id     INTEGER NOT NULL REFERENCES team(id) ON DELETE CASCADE,
    ord         INTEGER NOT NULL DEFAULT 0,
    role        TEXT NOT NULL,
    name        TEXT NOT NULL,
    purpose     TEXT NOT NULL DEFAULT '',

    -- Subscription-backed providers only (D8). The CHECK is the schema-level half of
    -- that rule; the machine profile enforces the other half at dispatch, because a
    -- provider allowed on the personal machine may be denied on the work one.
    provider    TEXT NOT NULL DEFAULT 'local'
                CHECK (provider IN ('claude','openai','zai','local')),
    model       TEXT NOT NULL,
    reasoning   TEXT NOT NULL DEFAULT 'medium'
                CHECK (reasoning IN ('none','minimal','low','medium','high','xhigh','max')),

    -- The paths this agent owns, one glob per line. Zone ownership is what lets the
    -- orchestrator dispatch a slice without asking anyone.
    zone        TEXT NOT NULL DEFAULT '',
    -- Either a named preset or a custom prompt, never both halves of the same question:
    -- a preset with an override is how prompt drift starts.
    prompt_preset TEXT,
    prompt_md     TEXT,
    -- A reviewer that can write is not a reviewer (maker != checker, pillar 3).
    read_only   INTEGER NOT NULL DEFAULT 0 CHECK (read_only IN (0,1)),
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),

    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL, context_window INTEGER,
    UNIQUE (team_id, role),
    CHECK (prompt_preset IS NOT NULL OR prompt_md IS NOT NULL)
);
INSERT INTO agent_rebuilt
    (id, team_id, ord, role, name, purpose, provider, model, reasoning, zone,
     prompt_preset, prompt_md, read_only, enabled, rev, created_at, updated_at,
     context_window)
SELECT
     id, team_id, ord, role, name, purpose, provider, model, reasoning, zone,
     prompt_preset, prompt_md, read_only, enabled, rev, created_at, updated_at,
     context_window
FROM agent;
DROP TABLE agent;
ALTER TABLE agent_rebuilt RENAME TO agent;
CREATE INDEX agent_team_ord ON agent(team_id, ord);

CREATE TABLE node_run_rebuilt (
    id            INTEGER PRIMARY KEY,
    run_id        INTEGER NOT NULL REFERENCES run(id) ON DELETE CASCADE,
    agent_id      INTEGER REFERENCES agent(id) ON DELETE SET NULL,
    -- Denormalised on purpose: an agent can be renamed or deleted, and a year-old run
    -- must still say who did the work.
    role          TEXT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,

    status        TEXT NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued','running','parked','blocked','done','failed','cancelled')),
    attempt       INTEGER NOT NULL DEFAULT 1,

    -- ai-planner's slice key, and ai-worktree's lease. Both are foreign systems' ids.
    slice_key     TEXT,
    worktree_path TEXT,
    branch        TEXT,
    lease_id      TEXT,
    -- eve's own session id, so a crashed supervisor can reattach rather than restart.
    session_id    TEXT,

    -- Cached and uncached are separate columns because a Claude node's ~64k prefix is
    -- cached after the first call, and totalling them would make every cold node look
    -- like a runaway (M1-S6, M3-S16).
    tokens_in          INTEGER NOT NULL DEFAULT 0,
    tokens_out         INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read  INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0,
    turns              INTEGER NOT NULL DEFAULT 0,

    blocked_reason TEXT,
    started_at    TEXT,
    ended_at      TEXT,
    rev           INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
, stream_cursor INTEGER NOT NULL DEFAULT 0, eve_port INTEGER, eve_token TEXT, context_tokens INTEGER, session_retired_at TEXT, session_resetting_at TEXT, pushed_at TEXT, pr_url TEXT, merge_requested_at TEXT, delivery_claim TEXT
    CHECK (delivery_claim IN ('push', 'pr', 'merge')), delivery_claimed_at TEXT, delivery_error TEXT, supervisor_pid INTEGER, task_key TEXT, push_replaces TEXT, pi_pid INTEGER, reasoning TEXT CHECK(reasoning IN ('none','minimal','low','medium','high','xhigh','max')));
INSERT INTO node_run_rebuilt
    (id, run_id, agent_id, role, provider, model, status, attempt, slice_key,
     worktree_path, branch, lease_id, session_id, tokens_in, tokens_out,
     tokens_cache_read, tokens_cache_write, turns, blocked_reason, started_at, ended_at,
     rev, created_at, updated_at, stream_cursor, eve_port, eve_token, context_tokens,
     session_retired_at, session_resetting_at, pushed_at, pr_url, merge_requested_at,
     delivery_claim, delivery_claimed_at, delivery_error, supervisor_pid, task_key,
     push_replaces, pi_pid, reasoning)
SELECT
     id, run_id, agent_id, role, provider, model, status, attempt, slice_key,
     worktree_path, branch, lease_id, session_id, tokens_in, tokens_out,
     tokens_cache_read, tokens_cache_write, turns, blocked_reason, started_at, ended_at,
     rev, created_at, updated_at, stream_cursor, eve_port, eve_token, context_tokens,
     session_retired_at, session_resetting_at, pushed_at, pr_url, merge_requested_at,
     delivery_claim, delivery_claimed_at, delivery_error, supervisor_pid, task_key,
     push_replaces, pi_pid, reasoning
FROM node_run;
DROP TABLE node_run;
ALTER TABLE node_run_rebuilt RENAME TO node_run;
CREATE INDEX node_run_run ON node_run(run_id, status);
CREATE INDEX node_run_slice ON node_run(slice_key);
CREATE INDEX node_run_worktree ON node_run(worktree_path);
CREATE TRIGGER chat_team_attempt_changed AFTER UPDATE ON node_run
WHEN EXISTS (SELECT 1 FROM chat_team_run WHERE run_id = NEW.run_id) BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = NEW.run_id);
END;
CREATE TRIGGER chat_push_revoke_turn AFTER UPDATE OF status ON node_run
WHEN NEW.status IN ('failed','cancelled')
BEGIN
    UPDATE chat_push_grant SET state='expired',result='The turn did not complete; this grant cannot be reused.',updated_at=NEW.updated_at
    WHERE node_id=NEW.id AND state IN ('pending','armed');
END;
CREATE TRIGGER node_reasoning_immutable BEFORE UPDATE OF reasoning ON node_run
BEGIN SELECT RAISE(ABORT, 'turn thinking is immutable'); END;

CREATE TABLE chat_rebuilt (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES project(id),
    title          TEXT NOT NULL,
    workspace_path TEXT NOT NULL,
    provider       TEXT NOT NULL CHECK (provider IN ('claude', 'openai', 'zai', 'local')),
    model          TEXT NOT NULL,
    reasoning      TEXT NOT NULL CHECK (reasoning IN ('none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max')),
    active_node_id INTEGER REFERENCES node_run(id),
    live_text      TEXT NOT NULL DEFAULT '',
    supervisor_identity TEXT,
    pi_identity    TEXT,
    stop_requested INTEGER NOT NULL DEFAULT 0 CHECK (stop_requested IN (0, 1)),
    archived       INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    rev            INTEGER NOT NULL DEFAULT 1,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
, mode TEXT NOT NULL DEFAULT 'single' CHECK (mode IN ('single', 'team')));
INSERT INTO chat_rebuilt
    (id, project_id, title, workspace_path, provider, model, reasoning, active_node_id,
     live_text, supervisor_identity, pi_identity, stop_requested, archived, rev,
     created_at, updated_at, mode)
SELECT
     id, project_id, title, workspace_path, provider, model, reasoning, active_node_id,
     live_text, supervisor_identity, pi_identity, stop_requested, archived, rev,
     created_at, updated_at, mode
FROM chat;
DROP TABLE chat;
ALTER TABLE chat_rebuilt RENAME TO chat;
CREATE INDEX chat_project ON chat(project_id, archived, updated_at);
CREATE UNIQUE INDEX chat_checkout_owner ON chat(workspace_path)
    WHERE active_node_id IS NOT NULL;
CREATE TRIGGER chat_push_revoke_chat AFTER UPDATE ON chat
WHEN NEW.archived != OLD.archived OR NEW.mode != OLD.mode
  OR NEW.workspace_path != OLD.workspace_path OR NEW.stop_requested = 1
  OR (NEW.active_node_id IS NOT NULL AND NEW.active_node_id IS NOT OLD.active_node_id)
BEGIN
    UPDATE chat_push_grant SET state='expired',result='Chat authority changed; this grant cannot be reused.',updated_at=NEW.updated_at
    WHERE chat_id=NEW.id AND state IN ('pending','armed');
END;

CREATE VIEW v_projects AS
SELECT
    p.id,
    p.slug,
    p.name,
    p.kind,
    p.status,
    t.name                                                                   AS team,
    (SELECT COUNT(*) FROM agent a WHERE a.team_id = p.team_id AND a.enabled = 1) AS agents,
    (SELECT COUNT(*) FROM project_repo pr WHERE pr.project_id = p.id)        AS repos,
    (SELECT COUNT(*) FROM run r WHERE r.project_id = p.id)                   AS runs,
    (SELECT COUNT(*) FROM run r WHERE r.project_id = p.id
                                 AND r.status IN ('queued','planning','running','blocked')) AS open_runs,
    (SELECT COUNT(*) FROM review v WHERE v.project_id = p.id AND v.status = 'open') AS open_reviews,
    (SELECT MAX(r.created_at) FROM run r WHERE r.project_id = p.id)          AS last_run_at,
    p.updated_at,
    p.created_at
FROM project p
LEFT JOIN team t ON t.id = p.team_id;
CREATE VIEW v_agents AS
SELECT
    a.id,
    p.slug  AS project,
    t.slug  AS team,
    a.ord,
    a.role,
    a.name,
    a.provider,
    a.model,
    a.reasoning,
    CASE a.read_only WHEN 1 THEN 'read-only' ELSE 'writes' END AS access,
    CASE a.enabled   WHEN 1 THEN 'enabled'   ELSE 'disabled' END AS state,
    a.zone,
    COALESCE(a.prompt_preset, 'custom') AS prompt,
    (SELECT COUNT(*) FROM agent_tool_policy tp WHERE tp.agent_id = a.id) AS tool_rules,
    a.purpose
FROM agent a
JOIN team t ON t.id = a.team_id
LEFT JOIN project p ON p.id = t.project_id;
CREATE VIEW v_events AS
SELECT
    e.id,
    e.at,
    p.slug AS project,
    e.run_id,
    n.role AS node,
    e.kind,
    e.actor,
    e.summary
FROM event e
JOIN run r ON r.id = e.run_id
JOIN project p ON p.id = r.project_id
LEFT JOIN node_run n ON n.id = e.node_run_id;
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
