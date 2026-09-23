-- A seat may be active in more than one checkout. A reply belongs to the exact
-- conversation it was written in, not whichever run for that seat asks first.
ALTER TABLE pending_message ADD COLUMN node_run_id INTEGER
    REFERENCES node_run(id) ON DELETE CASCADE;

DROP INDEX pending_message_waiting;
CREATE INDEX pending_message_waiting
    ON pending_message(agent_id, node_run_id, id)
    WHERE delivered_at IS NULL;
