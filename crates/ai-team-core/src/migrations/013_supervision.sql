-- A node can outlive the process that was supervising it. Recording the local
-- supervisor lets a restarted window distinguish "working" from "interrupted" and
-- atomically reattach the same Pi session and leased worktree.
ALTER TABLE node_run ADD COLUMN supervisor_pid INTEGER;
