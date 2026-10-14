-- Supervisor receipts are separate from untrusted provider payloads. Existing events
-- remain byte-for-byte intact and unobserved; the append-only triggers still apply.
ALTER TABLE event ADD COLUMN observation_json TEXT
    CHECK (observation_json IS NULL OR json_valid(observation_json));
