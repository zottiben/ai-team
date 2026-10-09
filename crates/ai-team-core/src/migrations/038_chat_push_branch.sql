-- A direct human task may explicitly request a branch which does not exist yet.
-- Keep the original branch/base evidence; bind the actual publication branch once,
-- together with its commit. Old current-branch grants retain their original meaning.
ALTER TABLE chat_push_grant ADD COLUMN new_branch_json TEXT
    CHECK (new_branch_json IS NULL OR json_valid(new_branch_json));
ALTER TABLE chat_push_grant ADD COLUMN pinned_branch TEXT;

CREATE TRIGGER chat_push_branch_insert BEFORE INSERT ON chat_push_grant
WHEN NEW.pinned_branch IS NOT NULL
  OR (NEW.new_branch_json IS NOT NULL
      AND (NEW.mode != 'single' OR NEW.node_id IS NULL OR NEW.commit_sha IS NOT NULL))
BEGIN SELECT RAISE(ABORT, 'new-branch push authority must start unpinned on its solo turn'); END;

CREATE TRIGGER chat_push_branch_immutable BEFORE UPDATE ON chat_push_grant
WHEN NEW.new_branch_json IS NOT OLD.new_branch_json
  OR (OLD.pinned_branch IS NOT NULL AND NEW.pinned_branch IS NOT OLD.pinned_branch)
  OR (NEW.pinned_branch IS NOT OLD.pinned_branch
      AND (OLD.state != 'pending' OR NEW.state != 'armed' OR NEW.commit_sha IS NULL
          OR (NEW.new_branch_json IS NULL AND NEW.pinned_branch IS NOT NEW.branch)))
  OR (NEW.new_branch_json IS NOT NULL AND NEW.state = 'armed' AND NEW.pinned_branch IS NULL)
BEGIN SELECT RAISE(ABORT, 'push branch scope and its pin are immutable'); END;
