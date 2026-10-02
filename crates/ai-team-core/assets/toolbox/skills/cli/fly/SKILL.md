---
name: fly-cli
description: Operate Fly.io via flyctl — deploy, status, logs, scale, secrets, machines. Use when working with a Fly-deployed app (checking a deploy, tailing logs, managing machines/secrets). Prefer this over a Fly MCP — flyctl already knows the API at zero context cost.
---

# fly — flyctl recipes

Read the app name and config from the repo (`**/fly/*.toml`) — never invent them.

- **Status / health:** `flyctl status -a <app>` · `flyctl checks list -a <app>`
- **Logs:** `flyctl logs -a <app>` (tail) · add `-i <machine>` to scope
- **Deploy:** `flyctl deploy --config <path/to/fly.toml>` — **deploy to STAGING
  first**, confirm, then prod on an explicit go.
- **Machines:** `flyctl machine list -a <app>` · `flyctl machine restart <id> -a <app>`
- **Secrets:** `flyctl secrets list -a <app>` (names only). Set with
  `flyctl secrets set K=V -a <app>` — values are sensitive; don't echo them.
- **Scale:** `flyctl scale show -a <app>` — changes cost money; be deliberate.

Safety: deploys and scaling are real and cost-bearing. Surface exactly what
you're about to run, default to staging, and stop before prod without an explicit
go.
