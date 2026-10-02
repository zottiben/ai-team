-- Human delivery approvals are separate from immutable build verification.
CREATE TABLE chat_delivery (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chat(id),
    run_id INTEGER NOT NULL REFERENCES chat_team_run(run_id),
    slice_key TEXT NOT NULL,
    workspace_path TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'preview' CHECK (state IN ('preview','running','done','refused','inspection','acknowledged')),
    supervisor_pid INTEGER,
    supervisor_identity TEXT,
    child_journal INTEGER NOT NULL DEFAULT 0 CHECK (child_journal IN (0,1)),
    attempted INTEGER NOT NULL DEFAULT 0 CHECK (attempted IN (0,1)),
    result TEXT,
    rev INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id, slice_key) REFERENCES chat_build_slice(run_id, slice_key)
);
CREATE UNIQUE INDEX chat_delivery_active ON chat_delivery(chat_id) WHERE state IN ('running','inspection');
CREATE TRIGGER chat_delivery_snapshot_immutable BEFORE UPDATE ON chat_delivery
WHEN NEW.chat_id != OLD.chat_id OR NEW.run_id != OLD.run_id OR NEW.slice_key != OLD.slice_key
  OR NEW.workspace_path != OLD.workspace_path OR NEW.snapshot_json != OLD.snapshot_json
  OR (OLD.attempted = 1 AND NEW.attempted != 1)
  OR (OLD.child_journal = 1 AND NEW.child_journal != 1)
  OR (OLD.state IN ('done','refused','acknowledged') AND (NEW.state != OLD.state OR NEW.result IS NOT OLD.result))
BEGIN SELECT RAISE(ABORT, 'delivery approval and settled evidence are immutable'); END;
