-- Project setup owns only explicitly registered roots, never the standalone registry.
CREATE TABLE toolbox_root (
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    PRIMARY KEY (project_id, path)
);

CREATE TABLE toolbox_preview (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    root TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'preview' CHECK (state IN ('preview','applying','applied','refused','partial')),
    outcome_json TEXT,
    created_at TEXT NOT NULL
);
-- A crash leaves applying inspection-only, not implicitly safe to retry.
CREATE UNIQUE INDEX toolbox_one_apply ON toolbox_preview(root) WHERE state = 'applying';
CREATE TRIGGER toolbox_snapshot_immutable BEFORE UPDATE OF project_id, root, snapshot_json ON toolbox_preview
BEGIN SELECT RAISE(ABORT, 'toolbox previews are immutable'); END;
