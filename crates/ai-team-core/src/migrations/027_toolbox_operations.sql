-- Extended approvals never confer project-to-HOME or checkout-to-checkout authority.
CREATE TABLE toolbox_operation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN ('user','converge','registry')),
    snapshot_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'preview' CHECK (state IN ('preview','applying','applied','refused','partial')),
    outcome_json TEXT,
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX toolbox_operation_apply ON toolbox_operation((1)) WHERE state='applying';
CREATE TRIGGER toolbox_operation_immutable BEFORE UPDATE OF kind,snapshot_json ON toolbox_operation
BEGIN SELECT RAISE(ABORT, 'toolbox operations are immutable'); END;
CREATE TABLE toolbox_scan_root (path TEXT PRIMARY KEY);
-- Forgetting hides a registration; restoring must not reopen paused/completed work.
CREATE TABLE toolbox_archived_project (
    project_id INTEGER PRIMARY KEY REFERENCES project(id),
    prior_status TEXT NOT NULL CHECK (prior_status IN ('active','paused','done'))
);
