-- A chat's next-turn preference is independent of the execution already in flight.
ALTER TABLE chat ADD COLUMN mode TEXT NOT NULL DEFAULT 'single' CHECK (mode IN ('single', 'team'));

-- The controller outlives individual Pi processes, including the approval pause.
CREATE TABLE chat_team_run (
    run_id               INTEGER PRIMARY KEY REFERENCES chat_turn(run_id),
    chat_id              INTEGER NOT NULL REFERENCES chat(id),
    control_node_id      INTEGER NOT NULL UNIQUE REFERENCES node_run(id),
    phase                TEXT NOT NULL CHECK (phase IN ('grounding', 'planning', 'awaiting_approval', 'building', 'blocked', 'finished')),
    supervisor_pid       INTEGER,
    supervisor_identity  TEXT,
    base_sha             TEXT,
    approved_revision    INTEGER,
    reason               TEXT,
    rev                  INTEGER NOT NULL DEFAULT 1
);

CREATE TRIGGER chat_team_run_binding BEFORE INSERT ON chat_team_run
WHEN NOT EXISTS (
    SELECT 1 FROM chat_turn t JOIN chat c ON c.id = t.chat_id JOIN run r ON r.id = t.run_id
    WHERE t.run_id = NEW.run_id AND t.chat_id = NEW.chat_id AND t.node_id = NEW.control_node_id
      AND r.project_id = c.project_id AND r.team_id IS NOT NULL
)
BEGIN SELECT RAISE(ABORT, 'a team execution must belong to its exact chat turn'); END;
CREATE TRIGGER chat_team_run_immutable BEFORE UPDATE OF run_id, chat_id, control_node_id ON chat_team_run
BEGIN SELECT RAISE(ABORT, 'team execution identity is immutable'); END;

-- Permissions and process identity are per attempt, not mutable team preferences or
-- a shared per-role MCP file. Nodes from another run cannot join this conversation.
CREATE TABLE chat_team_node (
    node_id       INTEGER PRIMARY KEY REFERENCES node_run(id),
    run_id        INTEGER NOT NULL REFERENCES chat_team_run(run_id),
    plan_access   TEXT NOT NULL CHECK (plan_access IN ('coordinator', 'planner', 'maker', 'reader')),
    pi_identity   TEXT,
    live_text     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX chat_team_node_run ON chat_team_node(run_id);
CREATE TRIGGER chat_team_node_binding BEFORE INSERT ON chat_team_node
WHEN NOT EXISTS (SELECT 1 FROM node_run n JOIN run r ON r.id = n.run_id JOIN agent a ON a.id = n.agent_id
                 WHERE n.id = NEW.node_id AND n.run_id = NEW.run_id AND a.team_id = r.team_id)
BEGIN SELECT RAISE(ABORT, 'a team member must belong to this execution and team'); END;
CREATE TRIGGER chat_team_node_immutable BEFORE UPDATE OF node_id, run_id, plan_access ON chat_team_node
BEGIN SELECT RAISE(ABORT, 'execution membership and permissions are immutable'); END;

-- References/fingerprints of the work a human approved, not copies of planner rows.
-- Lease state is execution evidence; the embedded planner remains the work graph.
CREATE TABLE chat_build_slice (
    run_id            INTEGER NOT NULL REFERENCES chat_team_run(run_id),
    slice_key         TEXT NOT NULL,
    planner_slice_id  INTEGER NOT NULL,
    approved_rev      INTEGER NOT NULL,
    worktree_path     TEXT,
    lease_holder      TEXT,
    branch            TEXT,
    lease_state       TEXT NOT NULL DEFAULT 'pending' CHECK (lease_state IN ('pending', 'leased', 'retained', 'released')),
    PRIMARY KEY (run_id, slice_key)
);

CREATE TRIGGER chat_team_run_changed AFTER UPDATE ON chat_team_run BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = NEW.chat_id;
END;
CREATE TRIGGER chat_team_node_changed AFTER UPDATE ON chat_team_node BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = NEW.run_id);
END;
CREATE TRIGGER chat_team_attempt_changed AFTER UPDATE ON node_run
WHEN EXISTS (SELECT 1 FROM chat_team_run WHERE run_id = NEW.run_id) BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = NEW.run_id);
END;
