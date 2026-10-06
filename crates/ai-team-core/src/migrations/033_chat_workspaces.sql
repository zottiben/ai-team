-- A request is a proposal, never permission for a running process to leave its lease.
CREATE TABLE chat_workspace_request (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chat(id),
    after_node_id INTEGER REFERENCES node_run(id),
    from_path TEXT NOT NULL,
    to_path TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','applied','cancelled')),
    created_at TEXT NOT NULL,
    settled_at TEXT
);
CREATE UNIQUE INDEX chat_workspace_pending ON chat_workspace_request(chat_id) WHERE state = 'pending';
CREATE TRIGGER chat_workspace_request_identity BEFORE UPDATE ON chat_workspace_request
WHEN NEW.chat_id != OLD.chat_id OR NEW.after_node_id IS NOT OLD.after_node_id
 OR NEW.from_path != OLD.from_path OR NEW.to_path != OLD.to_path OR NEW.created_at != OLD.created_at
 OR OLD.state != 'pending'
BEGIN SELECT RAISE(ABORT, 'workspace request identity and settled history are immutable'); END;
