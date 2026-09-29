-- Verification and delivery facts, never copies of the plan's definition/status.
ALTER TABLE chat_build_slice ADD COLUMN build_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (build_status IN ('pending', 'running', 'verified', 'failed', 'stopped'));
ALTER TABLE chat_build_slice ADD COLUMN candidate_sha TEXT;
ALTER TABLE chat_build_slice ADD COLUMN commit_sha TEXT
    CHECK (commit_sha IS NULL OR (candidate_sha IS NOT NULL AND commit_sha = candidate_sha));
ALTER TABLE chat_build_slice ADD COLUMN maker_node_id INTEGER REFERENCES node_run(id);
ALTER TABLE chat_build_slice ADD COLUMN verifier_node_id INTEGER REFERENCES node_run(id);
ALTER TABLE chat_build_slice ADD COLUMN release_started INTEGER NOT NULL DEFAULT 0 CHECK (release_started IN (0,1));

-- Recovery may acknowledge an exact publication intent, never replace its evidence.
CREATE TRIGGER chat_build_delivery_immutable BEFORE UPDATE OF candidate_sha, commit_sha, maker_node_id, verifier_node_id ON chat_build_slice
WHEN (OLD.candidate_sha IS NOT NULL AND (NEW.candidate_sha IS NOT OLD.candidate_sha OR NEW.maker_node_id IS NOT OLD.maker_node_id OR NEW.verifier_node_id IS NOT OLD.verifier_node_id))
    OR (OLD.commit_sha IS NOT NULL AND NEW.commit_sha IS NOT OLD.commit_sha)
BEGIN SELECT RAISE(ABORT, 'build publication evidence is immutable'); END;
