-- Latest context occupancy belongs to the resumable Pi session, not cumulative spend.
-- Retirement keeps the address and transcript as evidence while preventing future turns
-- from resuming it.
ALTER TABLE node_run ADD COLUMN context_tokens INTEGER;
ALTER TABLE node_run ADD COLUMN session_retired_at TEXT;
ALTER TABLE node_run ADD COLUMN session_resetting_at TEXT;
