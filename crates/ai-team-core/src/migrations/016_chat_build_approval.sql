-- External leasing and planner claims cannot share a SQLite commit. Keep intent and
-- intermediate states durable; only 'leased' grants a worker access to the plan.
ALTER TABLE chat_build_slice RENAME TO chat_build_slice_v15;
CREATE TABLE chat_build_slice (
    run_id            INTEGER NOT NULL REFERENCES chat_team_run(run_id),
    slice_key         TEXT NOT NULL,
    planner_slice_id  INTEGER NOT NULL,
    approved_rev      INTEGER NOT NULL,
    assigned_agent_id INTEGER,
    assigned_agent_rev INTEGER,
    agent_snapshot    TEXT NOT NULL DEFAULT '',
    worktree_path     TEXT,
    lease_holder      TEXT,
    branch            TEXT,
    lease_state       TEXT NOT NULL DEFAULT 'pending' CHECK (lease_state IN ('pending', 'acquiring', 'claiming', 'leased', 'retained', 'released')),
    reason            TEXT,
    rev               INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (run_id, slice_key)
);
INSERT INTO chat_build_slice (run_id, slice_key, planner_slice_id, approved_rev, worktree_path, lease_holder, branch, lease_state)
SELECT run_id, slice_key, planner_slice_id, approved_rev, worktree_path, lease_holder, branch, lease_state FROM chat_build_slice_v15;
DROP TABLE chat_build_slice_v15;

-- Agent ids are historical values, not FKs: deleting/recreating a seat must neither
-- rewrite an approval nor inherit it through a reused SQLite rowid. Compare the snapshot.
CREATE TRIGGER chat_build_approval_immutable BEFORE UPDATE OF run_id, slice_key, planner_slice_id, approved_rev, assigned_agent_id, assigned_agent_rev, agent_snapshot ON chat_build_slice
BEGIN SELECT RAISE(ABORT, 'build approval evidence is immutable'); END;
CREATE TRIGGER chat_build_slice_inserted AFTER INSERT ON chat_build_slice BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = NEW.run_id);
END;
CREATE TRIGGER chat_build_slice_changed AFTER UPDATE ON chat_build_slice BEGIN
    UPDATE chat SET rev = rev + 1 WHERE id = (SELECT chat_id FROM chat_team_run WHERE run_id = NEW.run_id);
END;
CREATE TRIGGER chat_build_base_immutable BEFORE UPDATE OF base_sha, approved_revision ON chat_team_run
WHEN OLD.approved_revision IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'the approved build base is immutable'); END;
