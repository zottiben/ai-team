-- Command pads a project owns, and what pressing one did.
--
-- Pads are the operator's own list: no seeded defaults, because a fixture command on a
-- one-press button is a command nobody chose. A pad is never typed into a shell - it is
-- run in a pty of its own - so `terminal_command` is the only record of an exit status,
-- and it keeps the command text it actually ran rather than pointing at a pad that may
-- have been edited since.
--
-- `revision` counts edits. `updated_at` cannot: it is second-precision, so two edits in
-- the same second read as no edit at all, and a press confirmed against the first would
-- be admitted against the second.
CREATE TABLE terminal_pad (
    id         INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    revision   INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    label      TEXT    NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 40),
    command    TEXT    NOT NULL CHECK (length(trim(command)) BETWEEN 1 AND 2000),
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX terminal_pad_project ON terminal_pad(project_id, position);

-- One row per press, written *before* the process starts.
--
-- `request_id` is minted by the window when the confirmation opens and is unique here, so
-- the row is the press: a repeated POST - a lost response, a reload, a double click -
-- finds this row and is handed the receipt it already has instead of starting the command
-- a second time. `fingerprint` is what was confirmed (project, checkout, pad, pad
-- revision, command text); a request id arriving with different bytes is a different press
-- wearing the same name and is refused rather than answered with the wrong receipt.
--
-- `dispatch` is why the row exists before the pty does:
--   intent   - recorded, and no process has been confirmed started. A process *may* exist.
--   started  - the pty is running and `session_id` names it.
--   refused  - the spawn failed with nothing left running. Provably nothing ran.
-- A row that stays `intent` is reported as an uncertain outcome and never re-run: the one
-- honest answer when a crash landed between the record and the spawn.
--
-- `token` is minted with the intent, not by the spawn, so an outcome observed later lands
-- on this run even if the write that was to record its session never completed.
CREATE TABLE terminal_command (
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
    -- Why nothing ran, on a refused row. The next press reads it rather than guessing.
    refusal     TEXT,
    session_id  INTEGER,
    claimed_at  TEXT    NOT NULL,
    started_at  TEXT,
    finished_at TEXT,
    exit_code   INTEGER,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    lines       INTEGER CHECK (lines IS NULL OR lines >= 0),
    -- False when the pty's scrollback discarded output before it was counted, so a line
    -- total is read as the floor it is.
    lines_exact INTEGER NOT NULL DEFAULT 1 CHECK (lines_exact IN (0, 1)),
    -- A started row names its pty and when it started; a refused one names neither.
    CHECK (dispatch <> 'started' OR (session_id IS NOT NULL AND started_at IS NOT NULL)),
    CHECK (dispatch <> 'refused' OR (session_id IS NULL AND refusal IS NOT NULL)),
    -- An outcome may land on a row still marked `intent`: that is the DB write recording
    -- the start having failed while the command itself ran and ended. Observed is
    -- observed, and it is recorded on the run its token names.
    CHECK (finished_at IS NOT NULL OR (exit_code IS NULL AND duration_ms IS NULL)),
    CHECK (dispatch <> 'refused' OR finished_at IS NULL)
);
CREATE INDEX terminal_command_history ON terminal_command(project_id, worktree, id DESC);
