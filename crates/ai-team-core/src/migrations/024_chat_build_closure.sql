-- Withdrawing approval is distinct from returning a lease. Keep this intent across
-- partial planner commits; an unfinished close must never authorize another maker.
CREATE TABLE chat_build_closure (
    run_id INTEGER PRIMARY KEY REFERENCES chat_team_run(run_id),
    reason TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    finished_at TEXT,
    issues_json TEXT NOT NULL DEFAULT '[]'
);
CREATE TRIGGER chat_build_closure_immutable BEFORE UPDATE ON chat_build_closure
WHEN NEW.run_id != OLD.run_id OR NEW.reason != OLD.reason OR NEW.requested_at != OLD.requested_at
    OR (OLD.finished_at IS NOT NULL AND (NEW.finished_at IS NOT OLD.finished_at OR NEW.issues_json != OLD.issues_json))
BEGIN SELECT RAISE(ABORT, 'build closure intent is immutable'); END;
CREATE TRIGGER chat_build_closure_no_delete BEFORE DELETE ON chat_build_closure
BEGIN SELECT RAISE(ABORT, 'build closure evidence cannot be deleted'); END;
