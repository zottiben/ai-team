-- A scheduled run needs somewhere to run, not something to say.
--
-- 001 required a prompt, on the reasoning that a run with nothing to submit would fire
-- and do nothing. Q14 changed what "nothing" means: `ait run -p <project>` with no prompt
-- builds whatever the plan already has ready, and that is the single most useful thing to
-- schedule. Requiring a prompt forces somebody to write out a description of the plan
-- they have already written, and the description is the copy that goes stale.
--
-- What a scheduled run genuinely cannot do without is a project. Without one there is
-- nowhere to lease a worktree and no plan to read, so that constraint replaces it - and
-- it fails at the moment somebody schedules the thing rather than at two in the morning.
--
-- SQLite cannot drop a CHECK, so the table is rebuilt. Foreign keys are suspended for the
-- swap because `reminder.project_id` points at `project`, which is untouched: re-enabling
-- afterwards re-validates, and the migration runner wraps this in a transaction.
PRAGMA foreign_keys = OFF;

-- Dropped first and rebuilt last: a view over a table that is about to disappear is left
-- dangling by the DROP, and every later query fails with "error in view v_due" rather
-- than anything about reminders.
DROP VIEW v_due;

CREATE TABLE reminder_new (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER REFERENCES project(id) ON DELETE CASCADE,
    team_id     INTEGER REFERENCES team(id) ON DELETE SET NULL,
    kind        TEXT NOT NULL DEFAULT 'reminder'
                CHECK (kind IN ('reminder','idea','scheduled_run')),
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    prompt      TEXT,
    due_at      TEXT,
    recur       TEXT CHECK (recur IS NULL OR recur IN ('daily','weekdays','weekly','monthly')),
    status      TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending','fired','done','cancelled')),
    last_fired_at TEXT,
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    -- A scheduled run with nowhere to run would fire and have nothing to lease.
    CHECK (kind <> 'scheduled_run' OR project_id IS NOT NULL)
);

INSERT INTO reminder_new
SELECT id, project_id, team_id, kind, title, body, prompt, due_at, recur, status,
       last_fired_at, rev, created_at, updated_at
  FROM reminder;

DROP TABLE reminder;
ALTER TABLE reminder_new RENAME TO reminder;
CREATE INDEX reminder_due ON reminder(status, due_at);

CREATE VIEW v_due AS
SELECT
    m.id,
    p.slug AS project,
    m.kind,
    m.title,
    m.due_at,
    m.recur,
    m.status,
    m.last_fired_at
FROM reminder m
LEFT JOIN project p ON p.id = m.project_id
WHERE m.status = 'pending' AND m.due_at IS NOT NULL;

PRAGMA foreign_keys = ON;
