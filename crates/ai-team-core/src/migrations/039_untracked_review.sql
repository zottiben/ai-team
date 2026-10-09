-- Extend the anchor area, preserving every finding and its original identity/evidence.
CREATE TABLE chat_checkout_finding_next (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 chat_id INTEGER NOT NULL REFERENCES chat(id),
 fingerprint TEXT NOT NULL,
 head TEXT,
 area TEXT NOT NULL CHECK(area IN ('staged','unstaged','untracked')),
 path TEXT NOT NULL,
 side TEXT NOT NULL CHECK(side IN ('old','new')),
 line INTEGER NOT NULL CHECK(line>0),
 body TEXT NOT NULL,
 created_at TEXT NOT NULL
);
INSERT INTO chat_checkout_finding_next SELECT * FROM chat_checkout_finding;
INSERT INTO sqlite_sequence(name,seq)
SELECT 'chat_checkout_finding_next',seq FROM sqlite_sequence
WHERE name='chat_checkout_finding' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='chat_checkout_finding_next');
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='chat_checkout_finding'),0))
WHERE name='chat_checkout_finding_next';
DROP TABLE chat_checkout_finding;
ALTER TABLE chat_checkout_finding_next RENAME TO chat_checkout_finding;
CREATE TRIGGER chat_checkout_finding_immutable BEFORE UPDATE ON chat_checkout_finding
BEGIN SELECT RAISE(ABORT,'checkout review evidence is immutable'); END;
