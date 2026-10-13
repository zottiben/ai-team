-- The MCP servers AI Team gives its own agents.
--
-- A chat's MCP discovery is explicit: generated planning and context servers, the
-- checkout's own definitions, and these. The operator's global Pi registrations are
-- deliberately not inherited, so a server reaches a seat because a row here says so.
-- An import copies a definition into this table as a pinned snapshot; it does not
-- create a link to the file it came from, and the source changing afterwards changes
-- nothing here.
--
-- `project_id` NULL is an app default, offered to every project. A row naming the same
-- server for one project replaces that default there - including replacing it with a
-- disabled row, which is how one project switches a default off without touching the
-- others.
CREATE TABLE mcp_server (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id         INTEGER REFERENCES project(id),
    -- The reserved names are the generated definitions a seat is held to: ai-team's own
    -- planner, the standalone one, and the two read-only context sources whose
    -- allow-lists are D15. Refused here as well as in the service, because a row that
    -- exists is a row some later code path may compose.
    name               TEXT NOT NULL CHECK (
                           length(name) BETWEEN 1 AND 64
                           AND name NOT IN ('ai-planner', 'ai-team-planner', 'clickup', 'figma')),
    -- Off until the operator says otherwise. Enabling one is what starts a local command
    -- or reaches a remote endpoint, so it is never the side effect of adding or importing.
    enabled            INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    transport          TEXT NOT NULL CHECK (transport IN ('stdio', 'http')),
    command            TEXT,
    args_json          TEXT NOT NULL DEFAULT '[]',
    url                TEXT,
    -- Values, and they are never returned to a surface. A turn gets them in its child
    -- environment under generated names; the generated config carries the placeholder,
    -- so a credential does not outlive the turn in a file beside the guard.
    env_json           TEXT NOT NULL DEFAULT '{}',
    headers_json       TEXT NOT NULL DEFAULT '{}',
    -- NULL is every tool this server exposes. A list is D15's allow-list, which filters
    -- discovery as well as calls.
    include_tools_json TEXT,
    origin             TEXT NOT NULL CHECK (origin IN ('manual', 'import')),
    -- What an imported row was pinned from, and when.
    origin_detail      TEXT NOT NULL DEFAULT '',
    rev                INTEGER NOT NULL DEFAULT 1,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    CHECK ((transport = 'stdio') = (command IS NOT NULL)),
    CHECK ((transport = 'http') = (url IS NOT NULL))
);
-- SQLite treats NULLs as distinct in a unique index, so the two scopes need their own.
CREATE UNIQUE INDEX mcp_server_default_name ON mcp_server(name) WHERE project_id IS NULL;
CREATE UNIQUE INDEX mcp_server_project_name ON mcp_server(project_id, name) WHERE project_id IS NOT NULL;
-- Identity is fixed, and every edit moves the revision. A surface that saved against an
-- older revision is refused rather than quietly overwriting somebody else's change.
CREATE TRIGGER mcp_server_identity BEFORE UPDATE ON mcp_server
WHEN NEW.id != OLD.id OR NEW.project_id IS NOT OLD.project_id OR NEW.name != OLD.name
  OR NEW.origin != OLD.origin OR NEW.created_at != OLD.created_at OR NEW.rev <= OLD.rev
BEGIN SELECT RAISE(ABORT, 'an MCP server''s identity is fixed; remove it and add it again'); END;
