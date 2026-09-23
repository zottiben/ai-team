-- Human/automatic boundaries for publishing accepted work.
ALTER TABLE team ADD COLUMN push_policy TEXT NOT NULL DEFAULT 'ask'
    CHECK (push_policy IN ('manual', 'ask', 'auto'));
ALTER TABLE team ADD COLUMN pr_policy TEXT NOT NULL DEFAULT 'ask'
    CHECK (pr_policy IN ('manual', 'ask', 'auto'));
ALTER TABLE team ADD COLUMN merge_policy TEXT NOT NULL DEFAULT 'ask'
    CHECK (merge_policy IN ('manual', 'ask', 'auto'));

-- Delivery state stays on the node that produced the exact branch. The append-only
-- event stream carries every attempt and failure; these columns are the current view.
ALTER TABLE node_run ADD COLUMN pushed_at TEXT;
ALTER TABLE node_run ADD COLUMN pr_url TEXT;
ALTER TABLE node_run ADD COLUMN merge_requested_at TEXT;
ALTER TABLE node_run ADD COLUMN delivery_claim TEXT
    CHECK (delivery_claim IN ('push', 'pr', 'merge'));
ALTER TABLE node_run ADD COLUMN delivery_claimed_at TEXT;
ALTER TABLE node_run ADD COLUMN delivery_error TEXT;
