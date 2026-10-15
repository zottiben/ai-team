-- New host observations only. No reconstructed precision or changes to old evidence.
ALTER TABLE event ADD COLUMN telemetry_json TEXT
    CHECK (telemetry_json IS NULL OR json_valid(telemetry_json));
-- Reading a seat's latest action must not scan hours of counter-only observations.
CREATE INDEX event_visible_node ON event(node_run_id, id DESC)
    WHERE telemetry_json IS NULL OR payload_json IS NOT NULL;
