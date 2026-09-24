-- A restack rewrites a pull request's branch onto its moved parent (PW11), so its push
-- cannot fast-forward. This is the one remote commit that push may replace: pinned when
-- the restack began, because a watch's own fetches move origin/<branch>, and a lease
-- checked against that would agree to replace a commit somebody pushed since. NULL is
-- every ordinary push, which must fast-forward.
ALTER TABLE node_run ADD COLUMN push_replaces TEXT;
