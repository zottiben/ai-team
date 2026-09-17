-- ai-team core schema: the org graph, and the runs it produces.
--
-- Two conventions run through the whole file, and both exist because this database is
-- browsed directly in TablePlus:
--
--   * timestamps are ISO-8601 UTC TEXT, second precision, so they sort lexically and
--     read without a decoder ring;
--   * statuses and kinds are lowercase words constrained by CHECK, so a typo is a
--     failed insert rather than a row nobody notices is wrong.
--
-- What is deliberately NOT here: the work graph. Plans and slices belong to ai-planner
-- (D4), and this schema only ever *references* them by their own keys. A column holding
-- a copy of a slice's title would be a second source of truth that drifts.

-- A project is a container, not a repo (D6). A quickfix that spans three services and a
-- ClickUp epic that spans none are both projects; the repos they touch hang off
-- project_repo, and there may be zero of them.
CREATE TABLE project (
    id           INTEGER PRIMARY KEY,
    slug         TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL DEFAULT 'repo'
                 CHECK (kind IN ('repo','ticket','epic','quickfix','triage','chore','adhoc')),
    status       TEXT NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active','paused','done','archived')),
    summary      TEXT,
    -- The brief as it arrived: a ClickUp description and its acceptance criteria, or
    -- whatever the human typed. Markdown, kept verbatim so an agent reads what was
    -- actually written rather than a lossy parse of it.
    brief_md     TEXT NOT NULL DEFAULT '',
    -- Where it came from, when it came from somewhere. Read-only by D9: ai-team never
    -- writes back to ClickUp or Figma.
    source       TEXT CHECK (source IS NULL OR source IN ('clickup','figma','manual')),
    source_key   TEXT,
    source_url   TEXT,
    -- The team currently running this project. Nullable, and the FK is the back half of
    -- a deliberate cycle with team.project_id: a project is created before its team
    -- exists, so this is set by a second statement. SQLite is fine with that.
    team_id      INTEGER,
    rev          INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE INDEX project_status ON project(status, kind);
CREATE INDEX project_source ON project(source, source_key);

-- Zero or more repos per project. `key` is the normalised remote, so the same repo
-- reached over ssh and https is one row.
CREATE TABLE project_repo (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    ord            INTEGER NOT NULL DEFAULT 0,
    key            TEXT NOT NULL,
    name           TEXT NOT NULL,
    remote_url     TEXT,
    main_path      TEXT,
    default_branch TEXT,
    created_at     TEXT NOT NULL,
    UNIQUE (project_id, key)
);

CREATE INDEX project_repo_key ON project_repo(key);

-- The team is the database (D2). Everything under .ai-team/agents/ is generated from
-- these rows, which is why there is no column here for a generated file's path.
--
-- project_id is nullable: a NULL team is a template, cloned onto a project rather than
-- run directly (M2-S7).
CREATE TABLE team (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER REFERENCES project(id) ON DELETE CASCADE,
    slug        TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',

    -- Guardrail defaults (M2-S10). These are the team's policy; a run copies them at
    -- dispatch (see run.*), so raising a budget tomorrow never rewrites what yesterday
    -- was allowed to spend.
    parallel_width    INTEGER NOT NULL DEFAULT 2 CHECK (parallel_width BETWEEN 1 AND 16),
    budget_tokens_run  INTEGER,
    budget_tokens_node INTEGER,
    budget_seconds_run  INTEGER,
    budget_seconds_node INTEGER,
    max_turns_node      INTEGER,
    -- How many times a verifier may bounce work back to its maker before the branch is
    -- given up on. Bounded on purpose: an unbounded repair loop is the failure mode.
    max_repairs         INTEGER NOT NULL DEFAULT 2,
    on_failure          TEXT NOT NULL DEFAULT 'retry'
                        CHECK (on_failure IN ('retry','escalate','abort_branch')),

    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (project_id, slug)
);

CREATE INDEX team_project ON team(project_id);

-- One row per seat. A node earns a seat only if it needs a different model, a different
-- tool surface, or is a read-only reviewer - so this table stays small by design, and
-- the default roster is six.
--
-- There is no `runtime` column. D7 settled it: every agent is an eve node, and a Claude
-- subscription arrives as a bridged *model provider*, not a second executor. Adding a
-- runtime column here would re-open a decision that has already been measured.
CREATE TABLE agent (
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
                CHECK (reasoning IN ('none','low','medium','high')),

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
    updated_at  TEXT NOT NULL,
    UNIQUE (team_id, role),
    CHECK (prompt_preset IS NOT NULL OR prompt_md IS NOT NULL)
);

CREATE INDEX agent_team_ord ON agent(team_id, ord);

-- The tool gateway, as rows. Deny beats allow, and an empty policy means the role
-- preset's default set - so a new agent is not accidentally omnipotent.
CREATE TABLE agent_tool_policy (
    id         INTEGER PRIMARY KEY,
    agent_id   INTEGER NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    tool       TEXT NOT NULL,
    effect     TEXT NOT NULL CHECK (effect IN ('allow','deny')),
    note       TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (agent_id, tool)
);

-- One prompt, one run. `plan_slug` points into ai-planner rather than copying its plan:
-- the work graph is theirs (D4).
CREATE TABLE run (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    team_id     INTEGER REFERENCES team(id) ON DELETE SET NULL,
    prompt      TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'queued'
                CHECK (status IN ('queued','planning','running','blocked','done','failed','cancelled')),
    trigger     TEXT NOT NULL DEFAULT 'manual'
                CHECK (trigger IN ('manual','scheduled','reminder','review')),
    plan_slug   TEXT,

    -- Snapshotted from the team at dispatch, never read back through the team. What a
    -- run was allowed to spend is a fact about that run.
    parallel_width  INTEGER NOT NULL DEFAULT 2,
    budget_tokens   INTEGER,
    budget_seconds  INTEGER,

    blocked_reason  TEXT,
    started_at  TEXT,
    ended_at    TEXT,
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX run_project_status ON run(project_id, status);
CREATE INDEX run_created ON run(created_at DESC);

-- One agent working one slice, in one leased worktree. A retry after a failed
-- verification is a new row with attempt + 1, not an edit - otherwise the evidence of
-- what went wrong the first time is gone, and that evidence is what analytics is made
-- of.
CREATE TABLE node_run (
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
);

CREATE INDEX node_run_run ON node_run(run_id, status);
CREATE INDEX node_run_slice ON node_run(slice_key);
CREATE INDEX node_run_worktree ON node_run(worktree_path);

-- Observability, append-only. This is the only record of what actually happened inside
-- a turn, so it is protected by a trigger rather than by everyone remembering not to
-- UPDATE it. Deletes are left alone so dropping a run still cascades.
CREATE TABLE event (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL REFERENCES run(id) ON DELETE CASCADE,
    node_run_id INTEGER REFERENCES node_run(id) ON DELETE CASCADE,
    at          TEXT NOT NULL,
    kind        TEXT NOT NULL
                CHECK (kind IN ('step','tool_call','tool_result','cost','approval_request',
                                'approval_resolved','build','note','done','failed')),
    actor       TEXT,
    -- A one-line human-readable summary, so the table is skimmable in TablePlus without
    -- expanding JSON on every row.
    summary     TEXT NOT NULL DEFAULT '',
    payload_json TEXT
);

CREATE INDEX event_run_at ON event(run_id, id);
CREATE INDEX event_node ON event(node_run_id, id);

CREATE TRIGGER event_is_append_only
BEFORE UPDATE ON event
BEGIN
    SELECT RAISE(ABORT, 'event is append-only');
END;

-- A diff waiting for a human. Anchored to the run that produced it and, when the work
-- came from one seat, to that node_run - which is what lets a submitted review steer
-- the responsible node instead of opening a fresh slice (M3-S15).
CREATE TABLE review (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    run_id      INTEGER REFERENCES run(id) ON DELETE SET NULL,
    node_run_id INTEGER REFERENCES node_run(id) ON DELETE SET NULL,
    title       TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open'
                CHECK (status IN ('open','changes_requested','approved','dismissed')),
    branch      TEXT,
    base_sha    TEXT,
    head_sha    TEXT,
    submitted_at TEXT,
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX review_project_status ON review(project_id, status);

-- `side` distinguishes a comment on a removed line from one on an added line at the
-- same number; without it a review reopened after a rebase lands its comments in the
-- wrong place.
CREATE TABLE review_comment (
    id          INTEGER PRIMARY KEY,
    review_id   INTEGER NOT NULL REFERENCES review(id) ON DELETE CASCADE,
    parent_id   INTEGER REFERENCES review_comment(id) ON DELETE CASCADE,
    file_path   TEXT,
    side        TEXT CHECK (side IS NULL OR side IN ('old','new')),
    line_start  INTEGER,
    line_end    INTEGER,
    author      TEXT NOT NULL DEFAULT 'human',
    body        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open'
                CHECK (status IN ('open','resolved','outdated')),
    created_at  TEXT NOT NULL,
    resolved_at TEXT
);

CREATE INDEX review_comment_review ON review_comment(review_id, file_path, line_start);

-- Reminders, the idea inbox, and scheduled run triggers are one table because they are
-- one question - "what should happen later" - and three tables would mean three places
-- to look when the answer is wrong. `kind` says which it is; `prompt` is what a
-- scheduled run would submit.
CREATE TABLE reminder (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER REFERENCES project(id) ON DELETE CASCADE,
    team_id     INTEGER REFERENCES team(id) ON DELETE SET NULL,
    kind        TEXT NOT NULL DEFAULT 'reminder'
                CHECK (kind IN ('reminder','idea','scheduled_run')),
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    prompt      TEXT,
    due_at      TEXT,
    -- Plain words rather than an RRULE: this is a scheduler for one person, and
    -- "weekdays" is both easier to read in TablePlus and easier to get right.
    recur       TEXT CHECK (recur IS NULL OR recur IN ('daily','weekdays','weekly','monthly')),
    status      TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending','fired','done','cancelled')),
    last_fired_at TEXT,
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    -- A scheduled run with nothing to submit would fire and do nothing.
    CHECK (kind <> 'scheduled_run' OR prompt IS NOT NULL)
);

CREATE INDEX reminder_due ON reminder(status, due_at);

-- Views exist so that opening the file in TablePlus answers "what is going on" with no
-- query written. They are part of the schema, not a convenience: the demo for this
-- slice is that the database explains itself.

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

CREATE VIEW v_runs AS
SELECT
    r.id,
    p.slug AS project,
    t.slug AS team,
    r.status,
    r.trigger,
    r.plan_slug,
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
    n.branch,
    n.worktree_path,
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

CREATE VIEW v_open_reviews AS
SELECT
    v.id,
    p.slug AS project,
    v.title,
    v.status,
    v.branch,
    (SELECT COUNT(*) FROM review_comment c WHERE c.review_id = v.id)                       AS comments,
    (SELECT COUNT(*) FROM review_comment c WHERE c.review_id = v.id AND c.status = 'open') AS unresolved,
    v.created_at
FROM review v
JOIN project p ON p.id = v.project_id
WHERE v.status IN ('open','changes_requested');

CREATE VIEW v_due AS
SELECT
    m.id,
    p.slug AS project,
    m.kind,
    m.title,
    m.due_at,
    m.recur,
    m.status,
    m.last_fired_at
FROM reminder m
LEFT JOIN project p ON p.id = m.project_id
WHERE m.status = 'pending' AND m.due_at IS NOT NULL;
