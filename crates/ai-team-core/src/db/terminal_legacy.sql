-- The terminal tables in the installed schema-49 database, before durable intents.
-- Test fixture only. The enclosing test database is disposable.
DROP TABLE terminal_command;
DROP TABLE terminal_pad;
CREATE TABLE terminal_pad (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    label TEXT NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 40),
    command TEXT NOT NULL CHECK (length(trim(command)) BETWEEN 1 AND 2000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX terminal_pad_project ON terminal_pad(project_id, position);
CREATE TABLE terminal_command (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    token TEXT NOT NULL UNIQUE,
    worktree TEXT NOT NULL,
    label TEXT NOT NULL,
    command TEXT NOT NULL,
    session_id INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    exit_code INTEGER,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    lines INTEGER CHECK (lines IS NULL OR lines >= 0),
    lines_exact INTEGER NOT NULL DEFAULT 1 CHECK (lines_exact IN (0, 1)),
    CHECK (finished_at IS NOT NULL OR (exit_code IS NULL AND duration_ms IS NULL))
);
CREATE INDEX terminal_command_history ON terminal_command(project_id, worktree, id DESC);
DROP INDEX IF EXISTS event_visible_node;
DELETE FROM schema_migrations WHERE version > 49;
