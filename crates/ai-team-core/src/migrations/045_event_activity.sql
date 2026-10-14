-- Host-owned, bounded activity snapshots; never deserialize these from Pi's payload.
-- Historical rows stay unknown and existing append-only/update guards stay intact.
ALTER TABLE event ADD COLUMN activity_json TEXT
    CHECK (activity_json IS NULL OR json_valid(activity_json));

-- stream_cursor may rewind on replay. A monotonic receipt prevents a fresh sampler
-- from backfilling previously suppressed updates, including after session continuation.
CREATE TABLE pi_ingest_cursor (
    session_id TEXT PRIMARY KEY,
    next_index INTEGER NOT NULL CHECK (next_index >= 0)
);
INSERT INTO pi_ingest_cursor(session_id, next_index)
SELECT session_id, MAX(MAX(stream_cursor, 0)) FROM node_run
WHERE session_id IS NOT NULL GROUP BY session_id;
