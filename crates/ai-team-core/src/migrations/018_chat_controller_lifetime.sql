-- Protocol 0 controllers know only a PID; never steal their work on a missing lock.
ALTER TABLE chat_team_run ADD COLUMN controller_protocol INTEGER NOT NULL DEFAULT 0 CHECK (controller_protocol IN (0,1));
-- Only ordinary, fully drained execution can certify this. A crash/aborted task cannot.
ALTER TABLE chat_team_run ADD COLUMN quiescent INTEGER NOT NULL DEFAULT 0 CHECK (quiescent IN (0,1));
