---
name: supabase-cli
description: Operate Supabase via the supabase CLI — migrations, type generation, local dev, schema diff. Use for schema/migration/codegen tasks; safer for writes than the Supabase MCP (which should stay read-only).
---

# supabase — CLI recipes

- **Migrations:** `supabase migration new <name>` → edit the SQL. Every new
  `public` table MUST `ENABLE ROW LEVEL SECURITY` (see the project's rules).
- **Types:** `supabase gen types typescript --project-id <ref> > <path>` (or the
  project's codegen command). Never hand-edit generated types.
- **Local dev:** `supabase start` / `stop` (Docker) · `supabase status` for local
  URLs/keys.
- **Diff:** `supabase db diff` to inspect schema drift.

Safety: never run destructive db commands against a remote/prod project. Keep the
access token / `service_role` key out of the transcript.
