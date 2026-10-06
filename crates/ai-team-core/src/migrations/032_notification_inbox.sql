-- Clearing dismisses attention, not its dedupe identity or underlying execution.
ALTER TABLE notification ADD COLUMN cleared_at TEXT;
CREATE INDEX idx_notification_inbox ON notification(id DESC) WHERE cleared_at IS NULL;

-- MAX(id) alone does not change when another window marks an alert read or clears it.
CREATE TABLE notification_inbox_revision (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO notification_inbox_revision (singleton) VALUES (1);
CREATE TRIGGER notification_inbox_insert AFTER INSERT ON notification BEGIN
    UPDATE notification_inbox_revision SET revision = revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER notification_inbox_delete AFTER DELETE ON notification BEGIN
    UPDATE notification_inbox_revision SET revision = revision + 1 WHERE singleton = 1;
END;
CREATE TRIGGER notification_inbox_update AFTER UPDATE OF read_at, cleared_at ON notification
WHEN OLD.read_at IS NOT NEW.read_at OR OLD.cleared_at IS NOT NEW.cleared_at BEGIN
    UPDATE notification_inbox_revision SET revision = revision + 1 WHERE singleton = 1;
END;
