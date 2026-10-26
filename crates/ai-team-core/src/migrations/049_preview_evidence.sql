-- Large presentation evidence is separate from snapshot_json so ordinary history
-- queries never hydrate every captured diff. Existing receipts remain unknown.
ALTER TABLE chat_checkout_operation ADD COLUMN preview_json TEXT
    CHECK (preview_json IS NULL OR (json_valid(preview_json) AND json_type(preview_json) = 'object'
        AND length(CAST(preview_json AS BLOB)) <= 524288));
ALTER TABLE chat_delivery ADD COLUMN preview_json TEXT
    CHECK (preview_json IS NULL OR (json_valid(preview_json) AND json_type(preview_json) = 'object'
        AND length(CAST(preview_json AS BLOB)) <= 524288));
CREATE TRIGGER chat_checkout_preview_immutable
BEFORE UPDATE OF preview_json ON chat_checkout_operation
WHEN NEW.preview_json IS NOT OLD.preview_json
BEGIN SELECT RAISE(ABORT, 'checkout preview evidence is immutable'); END;
CREATE TRIGGER chat_delivery_preview_immutable
BEFORE UPDATE OF preview_json ON chat_delivery
WHEN NEW.preview_json IS NOT OLD.preview_json
BEGIN SELECT RAISE(ABORT, 'delivery preview evidence is immutable'); END;
