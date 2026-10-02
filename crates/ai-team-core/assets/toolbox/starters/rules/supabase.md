# Supabase / Postgres — stack rules (starter snippet)

Common Supabase footguns. Delete what doesn't apply.

- **Enable RLS on every new `public` table.** Anything in `public` is reachable via
  PostgREST (anon/authenticated) unless RLS is on:
  `ALTER TABLE <t> ENABLE ROW LEVEL SECURITY;`. Default-deny, add policies as
  needed. (A server connecting as an owner/`postgres` role with BYPASSRLS is
  unaffected — the goal is closing the PostgREST surface.)
- **Pooler mode matters.** The transaction-mode pooler (Supavisor) breaks session
  features — `LISTEN/NOTIFY`, prepared statements, `SET`/session state. Use a
  session-mode / direct connection for those; the tx pooler is fine for stateless
  queries.
- **Verify the JWT, don't trust the client.** Validate Supabase JWTs against the
  project JWKS server-side; derive `user_id` from the verified token, never from
  request input.
- **Migrations are the source of truth.** Change schema via ordered, reversible
  migration files — not the dashboard. Generated types regenerate from schema;
  never hand-edit them.
- **`service_role` bypasses RLS** — server-side only, never shipped to a client.
- **Index filter/join/FK columns; watch N+1.** Use `explain analyze` on slow paths.
- **Storage & realtime honour RLS too** — configure their policies; don't assume open.
