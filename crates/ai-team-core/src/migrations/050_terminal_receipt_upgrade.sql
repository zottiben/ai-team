-- Both versions of the development schema used migration 47. Rebuild into one
-- canonical shape; db::terminal_receipts supplies the version-aware copy inside
-- the same transaction. Existing indexes/triggers are restored after the copy.
CREATE TABLE terminal_pad_next (
    id         INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    revision   INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    label      TEXT    NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 40),
    command    TEXT    NOT NULL CHECK (length(trim(command)) BETWEEN 1 AND 2000),
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE TABLE terminal_command_next (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    request_id  TEXT    NOT NULL UNIQUE,
    fingerprint TEXT    NOT NULL,
    token       TEXT    NOT NULL UNIQUE,
    worktree    TEXT    NOT NULL,
    pad_id      INTEGER NOT NULL,
    revision    INTEGER NOT NULL,
    label       TEXT    NOT NULL,
    command     TEXT    NOT NULL,
    dispatch    TEXT    NOT NULL CHECK (dispatch IN ('intent', 'started', 'refused')),
    refusal     TEXT,
    session_id  INTEGER,
    claimed_at  TEXT    NOT NULL,
    started_at  TEXT,
    finished_at TEXT,
    exit_code   INTEGER,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    lines       INTEGER CHECK (lines IS NULL OR lines >= 0),
    lines_exact INTEGER NOT NULL DEFAULT 1 CHECK (lines_exact IN (0, 1)),
    CHECK (dispatch <> 'started' OR (session_id IS NOT NULL AND started_at IS NOT NULL)),
    CHECK (dispatch <> 'refused' OR (session_id IS NULL AND refusal IS NOT NULL)),
    CHECK (finished_at IS NOT NULL OR (exit_code IS NULL AND duration_ms IS NULL)),
    CHECK (dispatch <> 'refused' OR finished_at IS NULL)
);
