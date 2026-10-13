-- Original image bytes belong to immutable human request identities, never checkout paths.
-- Keep small metadata before payload columns so polling need not walk image overflow pages.
ALTER TABLE chat_turn ADD COLUMN images_meta TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(images_meta) AND json_type(images_meta)='array');
ALTER TABLE chat_turn ADD COLUMN images_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(images_json) AND json_type(images_json)='array');
ALTER TABLE chat_followup ADD COLUMN images_meta TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(images_meta) AND json_type(images_meta)='array');
ALTER TABLE chat_followup ADD COLUMN images_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(images_json) AND json_type(images_json)='array');
CREATE TRIGGER chat_turn_images_immutable BEFORE UPDATE OF images_json,images_meta ON chat_turn
BEGIN SELECT RAISE(ABORT, 'chat image input is immutable'); END;
CREATE TRIGGER chat_followup_images_immutable BEFORE UPDATE OF images_json,images_meta ON chat_followup
BEGIN SELECT RAISE(ABORT, 'queued image input is immutable'); END;
