-- Operator saves made through the editor in a chat's own checkout (Q67).
--
-- Durable because the save happens in the window's process and the turn that is told
-- about it starts in another one, possibly minutes later. One row per chat per save, so
-- "already told" is a fact about a conversation rather than about a file.
--
-- Delivery is at the start of the next turn and nowhere else: nothing here reaches a
-- tool that is already running.
CREATE TABLE editor_save_notice (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chat(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    saved_at TEXT NOT NULL,
    -- The turn that was told, kept as evidence. Null until one is.
    delivered_node_id INTEGER REFERENCES node_run(id) ON DELETE SET NULL,
    delivered_at TEXT
);

CREATE INDEX editor_save_notice_pending
    ON editor_save_notice (chat_id, id) WHERE delivered_at IS NULL;
