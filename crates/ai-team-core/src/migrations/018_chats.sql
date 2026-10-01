-- A conversation survives its execution attempts. Nothing is imported from legacy runs.
CREATE TABLE chat (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES project(id),
    title          TEXT NOT NULL,
    workspace_path TEXT NOT NULL,
    provider       TEXT NOT NULL CHECK (provider IN ('claude', 'openai', 'zai', 'local')),
    model          TEXT NOT NULL,
    reasoning      TEXT NOT NULL CHECK (reasoning IN ('none', 'low', 'medium', 'high')),
    active_node_id INTEGER REFERENCES node_run(id),
    live_text      TEXT NOT NULL DEFAULT '',
    supervisor_identity TEXT,
    pi_identity    TEXT,
    stop_requested INTEGER NOT NULL DEFAULT 0 CHECK (stop_requested IN (0, 1)),
    archived       INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    rev            INTEGER NOT NULL DEFAULT 1,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
CREATE INDEX chat_project ON chat(project_id, archived, updated_at);
-- A stopped or interrupted supervisor does not release its checkout until reconciled.
CREATE UNIQUE INDEX chat_checkout_owner ON chat(workspace_path)
    WHERE active_node_id IS NOT NULL;

CREATE TABLE chat_turn (
    chat_id    INTEGER NOT NULL REFERENCES chat(id),
    run_id     INTEGER NOT NULL UNIQUE REFERENCES run(id),
    node_id    INTEGER NOT NULL UNIQUE REFERENCES node_run(id),
    request_id TEXT NOT NULL,
    PRIMARY KEY (chat_id, request_id)
);
CREATE INDEX chat_turn_history ON chat_turn(chat_id, run_id);

ALTER TABLE node_run ADD COLUMN pi_pid INTEGER;
