-- A queued instruction is not a legacy seat message or a delivered Pi prompt.
CREATE TABLE chat_followup (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id INTEGER NOT NULL REFERENCES chat(id),
    after_node_id INTEGER NOT NULL REFERENCES node_run(id),
    request_id TEXT NOT NULL,
    body TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('follow_up', 'steer')),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'starting', 'delivered', 'cancelled')),
    node_id INTEGER UNIQUE REFERENCES node_run(id),
    prompt_sha TEXT,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    UNIQUE(chat_id, request_id)
);
-- One pending instruction keeps its ordering and its exact predecessor unambiguous.
CREATE UNIQUE INDEX chat_followup_pending ON chat_followup(chat_id) WHERE state = 'queued';
