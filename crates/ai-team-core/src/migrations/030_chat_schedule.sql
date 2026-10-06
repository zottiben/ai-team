-- A scheduled prompt belongs to one exact chat and its checkout, never to a project's
-- latest run. The context recorded here is what the person saw when they scheduled it.
CREATE TABLE chat_schedule (
    id INTEGER PRIMARY KEY,
    reminder_id INTEGER NOT NULL UNIQUE REFERENCES reminder(id),
    chat_id INTEGER NOT NULL REFERENCES chat(id),
    project_id INTEGER NOT NULL REFERENCES project(id),
    workspace_path TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    reasoning TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('single', 'team')),
    created_at TEXT NOT NULL
);
CREATE INDEX chat_schedule_chat ON chat_schedule(chat_id);
-- Editing any of it would dispatch something else than what was agreed to, unattended.
CREATE TRIGGER chat_schedule_context_immutable BEFORE UPDATE ON chat_schedule
BEGIN SELECT RAISE(ABORT, 'a chat schedule is immutable; cancel it and make another'); END;

-- One row per claimed occurrence. The unique key is the at-most-once guarantee itself:
-- a second claimer of the same occurrence cannot insert, and never dispatches.
CREATE TABLE chat_schedule_occurrence (
    id INTEGER PRIMARY KEY,
    schedule_id INTEGER NOT NULL REFERENCES chat_schedule(id),
    occurrence_at TEXT NOT NULL,
    -- The chat turn's idempotency key, written before the turn exists, so a scheduler
    -- that died between claiming and dispatching is resolved by looking, not by guessing.
    request_id TEXT NOT NULL,
    claimed_at TEXT NOT NULL,
    claimed_pid INTEGER,
    claimed_identity TEXT,
    -- How many earlier occurrences the clock slept through before this one.
    skipped INTEGER NOT NULL DEFAULT 0,
    outcome TEXT NOT NULL DEFAULT 'claimed'
        CHECK (outcome IN ('claimed', 'started', 'busy', 'refused', 'failed', 'interrupted')),
    detail TEXT,
    run_id INTEGER REFERENCES run(id),
    node_id INTEGER REFERENCES node_run(id),
    settled_at TEXT,
    UNIQUE(schedule_id, occurrence_at)
);
CREATE INDEX chat_schedule_occurrence_schedule
    ON chat_schedule_occurrence(schedule_id, id DESC);
-- A settled occurrence is evidence of what the clock did. It is never retried and never
-- rewritten: an occurrence that is allowed to go back to 'claimed' is one that runs twice.
CREATE TRIGGER chat_schedule_occurrence_settled_once BEFORE UPDATE ON chat_schedule_occurrence
WHEN OLD.outcome != 'claimed' OR NEW.schedule_id != OLD.schedule_id
  OR NEW.occurrence_at != OLD.occurrence_at OR NEW.request_id != OLD.request_id
  OR NEW.claimed_at != OLD.claimed_at
BEGIN SELECT RAISE(ABORT, 'a settled schedule occurrence is immutable'); END;

-- A scheduled occurrence that never reached a turn still belongs to its chat, and has no
-- run to be derived from. Stated rows win; derived ones still answer for everything else.
ALTER TABLE notification ADD COLUMN chat_id INTEGER REFERENCES chat(id);
