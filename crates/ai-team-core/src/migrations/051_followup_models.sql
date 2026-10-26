-- New queued instructions capture the explicitly selected solo model. Historical
-- NULL means unrecorded, not permission to infer an account from a later preference.
ALTER TABLE chat_followup ADD COLUMN model_settings_json TEXT
    CHECK (model_settings_json IS NULL OR (json_valid(model_settings_json) AND json_type(model_settings_json) = 'object'));
CREATE TRIGGER chat_followup_model_immutable BEFORE UPDATE OF model_settings_json ON chat_followup
BEGIN SELECT RAISE(ABORT, 'queued model settings are immutable'); END;
