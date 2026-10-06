-- Human-requested AWT acquisition and setup, not an agent run or verified build.
CREATE TABLE workspace_setup (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL REFERENCES project(id),
 repo_path TEXT NOT NULL,
 request_id TEXT NOT NULL,
 branch TEXT NOT NULL,
 workspace_path TEXT,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','running','ready','failed','inspection')),
 detail TEXT NOT NULL DEFAULT '',
 child_epoch INTEGER NOT NULL DEFAULT 0,
 supervisor_pid INTEGER,
 rev INTEGER NOT NULL DEFAULT 1,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 UNIQUE(project_id,request_id)
);
CREATE UNIQUE INDEX workspace_setup_running ON workspace_setup(repo_path) WHERE state IN ('pending','running','inspection');
CREATE TRIGGER workspace_setup_identity BEFORE UPDATE ON workspace_setup
WHEN NEW.project_id!=OLD.project_id OR NEW.repo_path!=OLD.repo_path OR NEW.request_id!=OLD.request_id OR NEW.branch!=OLD.branch
 OR (OLD.workspace_path IS NOT NULL AND NEW.workspace_path IS NOT OLD.workspace_path)
 OR (OLD.state='ready' AND (NEW.state!=OLD.state OR NEW.detail!=OLD.detail))
BEGIN SELECT RAISE(ABORT,'workspace setup authority is immutable'); END;
CREATE TABLE workspace_setup_step (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 setup_id INTEGER NOT NULL REFERENCES workspace_setup(id),
 epoch INTEGER NOT NULL,
 command TEXT NOT NULL,
 passed INTEGER NOT NULL,
 output TEXT NOT NULL,
 at TEXT NOT NULL
);
CREATE TRIGGER workspace_setup_step_immutable BEFORE UPDATE ON workspace_setup_step
BEGIN SELECT RAISE(ABORT,'workspace setup evidence is immutable'); END;
CREATE TABLE workspace_setup_child (
 id INTEGER PRIMARY KEY,
 setup_id INTEGER NOT NULL REFERENCES workspace_setup(id),
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
CREATE INDEX workspace_setup_child_owner ON workspace_setup_child(setup_id,state);
CREATE TRIGGER workspace_setup_child_immutable BEFORE UPDATE ON workspace_setup_child
WHEN NEW.setup_id!=OLD.setup_id OR NEW.epoch!=OLD.epoch OR NEW.kind!=OLD.kind OR NEW.program!=OLD.program
 OR NEW.workspace IS NOT OLD.workspace OR NEW.boot!=OLD.boot
 OR (OLD.pid IS NOT NULL AND NEW.pid IS NOT OLD.pid) OR (OLD.identity IS NOT NULL AND NEW.identity IS NOT OLD.identity)
 OR (OLD.state='drained' AND NEW.state!='drained')
BEGIN SELECT RAISE(ABORT,'workspace setup child evidence is immutable'); END;
