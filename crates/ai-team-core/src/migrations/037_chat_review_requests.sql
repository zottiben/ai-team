-- A submitted review is one exact human request, not a deferred inbox or push grant.
CREATE TABLE chat_review_request (
    chat_id INTEGER NOT NULL REFERENCES chat(id),
    request_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    run_id INTEGER NOT NULL UNIQUE REFERENCES chat_turn(run_id),
    node_id INTEGER NOT NULL UNIQUE REFERENCES node_run(id),
    workspace_epoch INTEGER NOT NULL,
    source_head TEXT,
    parent_run_id INTEGER REFERENCES chat_team_run(run_id),
    slice_key TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (chat_id, request_id)
);
CREATE TRIGGER chat_review_request_binding BEFORE INSERT ON chat_review_request
WHEN NOT EXISTS (SELECT 1 FROM chat_turn t WHERE t.chat_id=NEW.chat_id AND t.run_id=NEW.run_id AND t.node_id=NEW.node_id)
 OR (NEW.parent_run_id IS NULL) != (NEW.slice_key IS NULL)
 OR (NEW.parent_run_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM chat_team_run t JOIN chat_build_slice s ON s.run_id=t.run_id
     WHERE t.chat_id=NEW.chat_id AND t.run_id=NEW.parent_run_id AND s.slice_key=NEW.slice_key))
BEGIN SELECT RAISE(ABORT, 'review dispatch must belong to its exact chat and draft'); END;
CREATE TRIGGER chat_review_request_immutable BEFORE UPDATE ON chat_review_request
BEGIN SELECT RAISE(ABORT, 'review dispatch evidence is immutable'); END;
CREATE TRIGGER chat_review_request_no_delete BEFORE DELETE ON chat_review_request
BEGIN SELECT RAISE(ABORT, 'review dispatch evidence is immutable'); END;

-- Older drafts without a scope receipt remain readable, but cannot silently authorise
-- repairs against a subsequently edited plan definition.
ALTER TABLE chat_build_slice ADD COLUMN scope_hash TEXT;
CREATE TRIGGER chat_build_scope_immutable BEFORE UPDATE OF scope_hash ON chat_build_slice
WHEN NEW.scope_hash IS NOT OLD.scope_hash
BEGIN SELECT RAISE(ABORT, 'approved scope evidence is immutable'); END;
