-- M9-S42: something said to a seat that was busy at the time.
--
-- eve let a message land in a turn already going: there was an HTTP session to post to.
-- Pi has no server - a turn is a child process reading one prompt - so a message arriving
-- mid-turn has nowhere to land, and the honest thing is to keep it and give it to that
-- seat's next turn, on the same session, so it arrives with the conversation behind it.
--
-- Deliberately not the event table. `event` is append-only and is the record of what
-- happened; this is a queue, and a row here stops being true the moment it is delivered.

CREATE TABLE pending_message (
    id         INTEGER PRIMARY KEY,
    agent_id   INTEGER NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    -- What was said, verbatim. It is somebody's words and is not rewritten.
    body       TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    -- Set when it reaches a turn. Kept rather than deleted so the window can say "that
    -- was delivered" rather than silently forgetting what somebody typed.
    delivered_at TEXT
);

-- The only query this table has: what is still waiting for one seat, oldest first, so two
-- messages arrive in the order they were said.
CREATE INDEX pending_message_waiting
    ON pending_message(agent_id, id)
    WHERE delivered_at IS NULL;
