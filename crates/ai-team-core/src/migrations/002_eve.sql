-- Ingesting eve's event stream without duplicating rows, and resuming it correctly.
--
-- eve's stream is durable and re-readable: a reconnect, a rewind to startIndex=0, or a
-- replay of a finished session all hand back events that were already seen. eve's own
-- documentation says to key ingestion on the event id and ignore conflicts, which is
-- what this migration makes possible.

-- The `evt_`-prefixed ULID from the event's `meta.id`.
--
-- Nullable on purpose: eve only began emitting `meta.id` in stream version 20, and it
-- passes older events through with the field absent rather than dropping them. Those
-- cannot be deduplicated, so they must still be insertable.
ALTER TABLE event ADD COLUMN eve_event_id TEXT;

-- A partial unique index, so the NULLs above do not collide with each other while any
-- real id can only land once. This is the whole dedupe mechanism: ingestion inserts
-- with OR IGNORE and re-reading a stream is free.
CREATE UNIQUE INDEX event_eve_id ON event(eve_event_id) WHERE eve_event_id IS NOT NULL;

-- How many events of this node's stream have been consumed.
--
-- eve's cursor is `startIndex`, an absolute count of events, and that is deliberately
-- what is stored rather than the last id: ULIDs are only broadly time-ordered, so
-- `WHERE id > last_seen` would silently skip or replay events. Reconnecting asks for
-- `?startIndex=<stream_cursor>`.
ALTER TABLE node_run ADD COLUMN stream_cursor INTEGER NOT NULL DEFAULT 0;

-- Where the generated eve project for this run's team was written, and the port its
-- supervised process was given. Recorded per node because D10 gives each leased
-- worktree its own `eve start`, so two concurrent nodes are two processes.
ALTER TABLE node_run ADD COLUMN eve_port INTEGER;

-- What context window this agent's model has, in tokens.
--
-- eve refuses to compile compaction for a model it cannot size, and it can only size
-- AI Gateway model IDs. Every provider ai-team can use is a direct or openai-compatible
-- one (D8), so eve will never know - the number has to come from here. NULL means "not
-- established yet"; the generator falls back to a deliberately small default, because
-- compacting earlier than necessary is survivable and overflowing the window is not.
ALTER TABLE agent ADD COLUMN context_window INTEGER;

-- v_node_runs gains the cursor, so a stalled stream is visible in TablePlus without
-- writing a query. SQLite cannot ALTER a view, so it is dropped and recreated.
DROP VIEW v_node_runs;

CREATE VIEW v_node_runs AS
SELECT
    n.id,
    p.slug AS project,
    n.run_id,
    n.role,
    n.provider,
    n.model,
    n.status,
    n.attempt,
    n.slice_key,
    n.branch,
    n.worktree_path,
    n.eve_port,
    n.stream_cursor,
    (SELECT COUNT(*) FROM event e WHERE e.node_run_id = n.id) AS events,
    n.turns,
    n.tokens_in,
    n.tokens_out,
    n.tokens_cache_read,
    n.tokens_cache_write,
    n.blocked_reason,
    n.started_at,
    n.ended_at
FROM node_run n
JOIN run r ON r.id = n.run_id
JOIN project p ON p.id = r.project_id;
