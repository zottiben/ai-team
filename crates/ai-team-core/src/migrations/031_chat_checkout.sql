-- Operator actions are not agent turns or verified team drafts.
CREATE TABLE chat_checkout_operation (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 chat_id INTEGER NOT NULL REFERENCES chat(id),
 workspace_path TEXT NOT NULL,
 snapshot_json TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'preview' CHECK(state IN ('preview','running','done','refused','inspection','acknowledged')),
 result TEXT,
 attempted INTEGER NOT NULL DEFAULT 0,
 child_epoch INTEGER NOT NULL DEFAULT 0,
 supervisor_pid INTEGER,
 rev INTEGER NOT NULL DEFAULT 1,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);
CREATE TRIGGER chat_checkout_immutable BEFORE UPDATE ON chat_checkout_operation
WHEN NEW.chat_id != OLD.chat_id OR NEW.workspace_path != OLD.workspace_path OR NEW.snapshot_json != OLD.snapshot_json
 OR (OLD.attempted=1 AND NEW.attempted!=1)
 OR (OLD.state IN ('done','refused','acknowledged') AND (NEW.state!=OLD.state OR NEW.result IS NOT OLD.result))
BEGIN SELECT RAISE(ABORT,'checkout approval and outcome are immutable'); END;
CREATE TABLE checkout_child (
 id INTEGER PRIMARY KEY,
 operation_id INTEGER NOT NULL REFERENCES chat_checkout_operation(id),
 epoch INTEGER NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('pi','command')),
 program TEXT NOT NULL,
 workspace TEXT,
 boot TEXT NOT NULL,
 pid INTEGER CHECK(pid IS NULL OR pid>0),
 identity TEXT,
 state TEXT NOT NULL DEFAULT 'intent' CHECK(state IN ('intent','running','drained')),
 created_at TEXT NOT NULL,
 ended_at TEXT,
 CHECK(identity IS NULL OR pid IS NOT NULL),
 CHECK(state!='running' OR (pid IS NOT NULL AND identity IS NOT NULL))
);
CREATE INDEX checkout_child_operation ON checkout_child(operation_id,state);
CREATE TRIGGER checkout_child_immutable BEFORE UPDATE ON checkout_child
WHEN NEW.operation_id!=OLD.operation_id OR NEW.epoch!=OLD.epoch OR NEW.kind!=OLD.kind OR NEW.program!=OLD.program
 OR NEW.workspace IS NOT OLD.workspace OR NEW.boot!=OLD.boot
 OR (OLD.pid IS NOT NULL AND NEW.pid IS NOT OLD.pid) OR (OLD.identity IS NOT NULL AND NEW.identity IS NOT OLD.identity)
 OR (OLD.state='drained' AND NEW.state!='drained')
BEGIN SELECT RAISE(ABORT,'checkout child evidence is immutable'); END;
CREATE TABLE chat_checkout_finding (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 chat_id INTEGER NOT NULL REFERENCES chat(id),
 fingerprint TEXT NOT NULL,
 head TEXT,
 area TEXT NOT NULL CHECK(area IN ('staged','unstaged')),
 path TEXT NOT NULL,
 side TEXT NOT NULL CHECK(side IN ('old','new')),
 line INTEGER NOT NULL CHECK(line>0),
 body TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE TRIGGER chat_checkout_finding_immutable BEFORE UPDATE ON chat_checkout_finding
BEGIN SELECT RAISE(ABORT,'checkout review evidence is immutable'); END;
