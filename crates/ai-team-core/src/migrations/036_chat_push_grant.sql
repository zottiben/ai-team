-- Explicit human push authority, and nothing else.
--
-- Only a direct, authenticated human message in a chat mints one row here: never an
-- agent, a schedule, a queued instruction, a review body or imported text. A row
-- authorises one push, of one branch, in one chat, on one origin, once. It is not
-- permission to open a pull request, merge, tag or force.
CREATE TABLE chat_push_grant (
    id                  INTEGER PRIMARY KEY,
    chat_id             INTEGER NOT NULL REFERENCES chat(id),
    -- The exact human request that said it. A replay of that request finds this row.
    request_id          TEXT NOT NULL,
    -- The solo turn allowed to pin a commit. NULL for a team draft, which is already
    -- committed and verified, so no turn is started for it.
    node_id             INTEGER REFERENCES node_run(id),
    draft_json          TEXT,
    mode                TEXT NOT NULL CHECK (mode IN ('single', 'team')),
    message             TEXT NOT NULL,
    workspace_path      TEXT NOT NULL,
    workspace_epoch     INTEGER NOT NULL,
    branch              TEXT NOT NULL,
    origin_url          TEXT NOT NULL,
    -- What the branch pointed at when the person asked.
    head_sha            TEXT NOT NULL,
    allow_commit        INTEGER NOT NULL DEFAULT 0 CHECK (allow_commit IN (0, 1)),
    -- The process the person was talking to. A restart does not inherit its authority.
    supervisor_pid      INTEGER NOT NULL,
    supervisor_identity TEXT NOT NULL,
    -- Pinned once, after the authorised commit exists.
    commit_sha          TEXT,
    -- The journalled checkout operation that published it. Set once, so a replay
    -- inspects that receipt instead of pushing again.
    operation_id        INTEGER REFERENCES chat_checkout_operation(id),
    state               TEXT NOT NULL DEFAULT 'pending'
                        CHECK (state IN ('pending', 'armed', 'spent', 'expired')),
    result              TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE UNIQUE INDEX chat_push_grant_request ON chat_push_grant(chat_id, request_id);
-- One live authority per chat. A second message cannot stack another pending push, and a
-- stale one has to be expired before a new one is minted.
CREATE UNIQUE INDEX chat_push_grant_live ON chat_push_grant(chat_id)
    WHERE state IN ('pending', 'armed');
CREATE INDEX chat_push_grant_node ON chat_push_grant(node_id, state);
CREATE TRIGGER chat_push_grant_immutable BEFORE UPDATE ON chat_push_grant
WHEN NEW.chat_id != OLD.chat_id OR NEW.request_id != OLD.request_id
  OR NEW.node_id IS NOT OLD.node_id OR NEW.draft_json IS NOT OLD.draft_json OR NEW.mode != OLD.mode OR NEW.message != OLD.message
  OR NEW.workspace_path != OLD.workspace_path OR NEW.workspace_epoch != OLD.workspace_epoch
  OR NEW.branch != OLD.branch OR NEW.origin_url != OLD.origin_url
  OR NEW.head_sha != OLD.head_sha OR NEW.allow_commit != OLD.allow_commit
  OR NEW.supervisor_pid != OLD.supervisor_pid
  OR NEW.supervisor_identity != OLD.supervisor_identity OR NEW.created_at != OLD.created_at
  OR (OLD.commit_sha IS NOT NULL AND NEW.commit_sha IS NOT OLD.commit_sha)
  OR (OLD.operation_id IS NOT NULL AND NEW.operation_id IS NOT OLD.operation_id)
  OR (OLD.state = 'armed' AND NEW.state = 'pending')
  OR (OLD.state IN ('spent', 'expired')
      AND (NEW.state != OLD.state OR NEW.result IS NOT OLD.result))
BEGIN SELECT RAISE(ABORT, 'push authority and its outcome are immutable'); END;

-- Revocation is durable. Clearing Stop, restoring an archive or changing mode back
-- must not make a stale grant live again.
CREATE TRIGGER chat_push_revoke_chat AFTER UPDATE ON chat
WHEN NEW.archived != OLD.archived OR NEW.mode != OLD.mode
  OR NEW.workspace_path != OLD.workspace_path OR NEW.stop_requested = 1
  OR (NEW.active_node_id IS NOT NULL AND NEW.active_node_id IS NOT OLD.active_node_id)
BEGIN
    UPDATE chat_push_grant SET state='expired',result='Chat authority changed; this grant cannot be reused.',updated_at=NEW.updated_at
    WHERE chat_id=NEW.id AND state IN ('pending','armed');
END;
CREATE TRIGGER chat_push_revoke_turn AFTER UPDATE OF status ON node_run
WHEN NEW.status IN ('failed','cancelled')
BEGIN
    UPDATE chat_push_grant SET state='expired',result='The turn did not complete; this grant cannot be reused.',updated_at=NEW.updated_at
    WHERE node_id=NEW.id AND state IN ('pending','armed');
END;
CREATE TRIGGER chat_push_revoke_workspace AFTER UPDATE OF state ON chat_workspace_request
WHEN NEW.state='applied' AND OLD.state!='applied'
BEGIN
    UPDATE chat_push_grant SET state='expired',result='The checkout generation changed; this grant cannot be reused.',updated_at=NEW.settled_at
    WHERE chat_id=NEW.chat_id AND state IN ('pending','armed');
END;
