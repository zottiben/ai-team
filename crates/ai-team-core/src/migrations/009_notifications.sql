-- Durable attention deliveries. Run/event state remains authoritative; these rows only
-- say that one transition should be brought to the operator's attention once.
CREATE TABLE notification (
    id              INTEGER PRIMARY KEY,
    dedupe_key      TEXT NOT NULL UNIQUE,
    project_id      INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    workspace_path  TEXT,
    run_id          INTEGER REFERENCES run(id) ON DELETE CASCADE,
    node_run_id     INTEGER REFERENCES node_run(id) ON DELETE SET NULL,
    kind            TEXT NOT NULL
                    CHECK (kind IN ('plan_ready','completed','failed','input_required','follow_up')),
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    action_path     TEXT,
    read_at         TEXT,
    delivery_claimed_at TEXT,
    delivered_at    TEXT,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_notification_created ON notification(id DESC);
CREATE INDEX idx_notification_unread ON notification(read_at, id DESC);
CREATE INDEX idx_notification_delivery
    ON notification(delivered_at, delivery_claimed_at, id);
