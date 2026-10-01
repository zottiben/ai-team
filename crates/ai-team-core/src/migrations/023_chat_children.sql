-- Only journal-aware controllers may certify abandoned children as drained.
ALTER TABLE chat_team_run ADD COLUMN child_journal INTEGER NOT NULL DEFAULT 0 CHECK (child_journal IN (0,1));
ALTER TABLE chat_team_run ADD COLUMN child_epoch INTEGER NOT NULL DEFAULT 0;
CREATE TABLE chat_child (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES chat_team_run(run_id),
    epoch INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('pi','command')),
    program TEXT NOT NULL,
    workspace TEXT,
    boot TEXT NOT NULL,
    pid INTEGER CHECK (pid IS NULL OR pid > 0),
    identity TEXT,
    state TEXT NOT NULL DEFAULT 'intent' CHECK (state IN ('intent','running','drained')),
    created_at TEXT NOT NULL,
    ended_at TEXT,
    CHECK (identity IS NULL OR pid IS NOT NULL),
    CHECK (state != 'running' OR (pid IS NOT NULL AND identity IS NOT NULL))
);
CREATE INDEX chat_child_run ON chat_child(run_id, state);
CREATE TRIGGER chat_child_identity_immutable BEFORE UPDATE ON chat_child
WHEN NEW.run_id != OLD.run_id OR NEW.epoch != OLD.epoch OR NEW.kind != OLD.kind
  OR NEW.program != OLD.program OR NEW.workspace IS NOT OLD.workspace OR NEW.boot != OLD.boot
  OR (OLD.pid IS NOT NULL AND NEW.pid IS NOT OLD.pid)
  OR (OLD.identity IS NOT NULL AND NEW.identity IS NOT OLD.identity)
  OR (OLD.state = 'drained' AND NEW.state != 'drained')
BEGIN SELECT RAISE(ABORT, 'child identity and drained evidence are immutable'); END;
