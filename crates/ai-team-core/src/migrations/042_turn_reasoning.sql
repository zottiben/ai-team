-- Historical turns did not record their requested thinking level. NULL means unknown,
-- not the chat/team's current preference. New attempts snapshot it at admission.
ALTER TABLE node_run ADD COLUMN reasoning TEXT CHECK(reasoning IN ('none','low','medium','high'));
CREATE TRIGGER node_reasoning_immutable BEFORE UPDATE OF reasoning ON node_run
BEGIN SELECT RAISE(ABORT, 'turn thinking is immutable'); END;
