-- Old running processes must not dispatch or mutate state after a newer release
-- has been installed. Reads remain available so drafts can be saved before restart.
CREATE TABLE app_update (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    version TEXT NOT NULL,
    backup_path TEXT NOT NULL,
    installed_at TEXT NOT NULL
);
