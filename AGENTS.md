# ai-team — project knowledge

A local desktop platform where one prompt to an orchestrator is planned and built by a
configurable team of AI agents, working through the ai-planner board. For one engineer,
on their own machine.

**Stack:** Rust 1.98.1 (pinned), edition 2021, workspace shaped like `ai-planner`.
React 19 + Vite 8 + TypeScript, built to static assets and embedded in the binary.
Agents run as Vercel **eve** nodes (Node 24+).

**Platforms — macOS is the target, Linux is the dev machine (D12).** ai-team is used daily
on macOS; it is built on Linux. Both must work, and where they disagree macOS wins. The
author cannot hand-test the primary platform, so CI's `macos-latest` leg is the real
verification — put platform-sensitive logic behind a test that runs on **both** legs
rather than checking it by hand. Three macOS facts bite (see the gotcha in the plan):
`/tmp` and `/var` are symlinks into `/private`, APFS is case-insensitive by default, and
the login shell is zsh.

**Layout**
- `crates/ai-team-core` — the org graph, the store, run state. No deps on the surfaces.
- `crates/ai-team-ui` — axum server on loopback + the embedded bundle. `build.rs` compiles
  `ui/dist` in.
- `crates/ai-team` — the CLI. Binary is **`ait`**, not `ai-team`.
- `crates/ai-team-desktop` — Tauri shell, binary `ai-team`. **Out of `default-members`.**
- `ui/` — the frontend. `ui/dist` is **committed**; CI fails if it is stale.
- `design/concept.md` — binding concept: pillars, anti-pillars, scope tiers
- `install/install.sh` — published to `zottiben.github.io/ai-team` by `pages.yml`

**Commands**

| What | Command |
| --- | --- |
| Build / test / lint | `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` |
| Format | `cargo fmt --all` (`--check` in CI) |
| Supply chain | `cargo deny check` (needs `cargo install cargo-deny --locked`) |
| Desktop shell | `cargo clippy -p ai-team-desktop --all-targets -- -D warnings` |
| Frontend | `cd ui && npm ci && npm run typecheck && npm test && npm run build` |
| Run it | `ait init`, `ait doctor`, `./target/debug/ait ui --port 7788 --no-open` |
| Drive an agent | `ait agents set-model …`, then `ait run -p <project> --worktree <dir> "…"` |
| The database | `ait db path`, `ait db open` (TablePlus), `ait db views` |
| Icons | `cd crates/ai-team-desktop/icons && sh regenerate.sh` |

The root `cargo` commands skip `ai-team-desktop` on purpose — it pulls in Tauri and a
platform webview. CI compiles it separately so it cannot rot. On Linux it needs the
webview headers listed in `.github/workflows/ci.yml`.

## Plan and design

There is no `BUILD_PLAN.md` or `HANDOFF.md`. The plan is a row in ai-planner:

```sh
aip status            # where you are, what is next
aip show -p ai-team   # the whole plan: decisions, gotchas, slices
aip resume            # after a context clear
```

`design/concept.md` is the source of truth for scope. Build against it; when a decision
isn't covered, ask and record it with `aip decision add`.

## Hard rules

Nine decisions are recorded in the plan (`aip decision ls`). These five are the ones an
agent will otherwise get wrong, so they are repeated here.

### 1. Subscription-backed models only (D8)
The entire point is to stop managing balances across several accounts, and the work machine
forbids some providers outright. Never introduce a metered API key path.

| Provider | Auth |
| --- | --- |
| Claude | Claude subscription, via the OAuthed Claude Code CLI |
| OpenAI | ChatGPT subscription, via eve `/login` |
| z.ai GLM | GLM Coding Plan (flat), openai-compatible at `https://api.z.ai/api/coding/paas/v4` |
| local | free, openai-compatible at the ailocal gateway on `127.0.0.1:8081` |

A machine profile (`~/.config/ai-team/machine.toml`) allows or denies each provider and is
enforced **at dispatch**, not only in the UI picker — a scheduled unattended run must not be
able to reach a denied provider.

### 2. One runtime: eve (D7)
Every agent is an eve node. There is no second executor and no Pi. A Claude-subscription
agent is an eve node whose model is bridged:

```ts
model: claudeCode('sonnet', {
  mcpServers: { eve: createAiSdkMcpServer('eve', tools) },
  allowedTools: ['mcp__eve__*'],
  settingSources: [],     // NOT undefined — undefined inherits the human's Claude config
})
```

This only works because we generate the eve project, so the generated `agent.ts` can import
the very tool modules it bridges. eve's *built-in* tools have no importable `execute` and
cannot be bridged — disable them on these nodes.

### 3. The database is the team; the eve project is generated (D2)
Never hand-edit anything under `.ai-team/agents/`. Change the team rows and regenerate.

The store (`ai-team-core`) is the **only** place that writes SQL — the CLI, the server and
the desktop shell all go through it. The schema is one file, `migrations/001_core.sql`,
`include_str!`d so a `cargo install`ed binary carries it. Two properties it enforces that
are easy to undo by accident:

- **`event` is append-only**, by trigger. It is the only record of what happened inside a
  turn, so there is no `update_event` and there must never be one.
- **A run snapshots its team's guardrails** at dispatch. Never read a budget back through
  `team` — what a run was allowed to spend is a fact about that run.

Also: a retry is a **new `node_run` row** (attempt + 1), never an edit. The first attempt's
evidence is what analytics is made of. And `run.plan_slug` / `node_run.slice_key` are
*references* into ai-planner — never copy a plan or slice into this database (D4).

### 4. Agents edit leased worktrees, not eve's sandbox (D3)
Authored `bash`/`read_file`/`write_file`/`edit_file` tools act on a worktree leased with
`awt get --lease`. eve's sandbox is for genuinely untrusted execution only.

**One eve process per leased worktree (D10).** eve has no way for a client to attach
per-session metadata that reaches a tool, so the worktree is bound at the process level
via `$AI_TEAM_WORKTREE`. Isolation is an OS fact, not a check.

The guard lives in exactly one file, `generate/assets/lib/worktree.ts`, and it is
`node --test`ed on **both** CI legs because its macOS behaviour is what the author cannot
see (D12). `read_only` seats simply don't get the write tools generated — but they do get
`bash`, so the guarantee is "cannot edit source through its tools", not "cannot write a
byte".

Four things about eve that cost a build each to learn:
- `defaultTools: false` removes the whole optional default set. `disableTool()` at a slot
  with no framework default **underneath** it is a build error — to withhold a tool, omit
  the file.
- `eve build` **evaluates** every authored module, so a module-scope `throw` for a missing
  env var fails the build on any machine that isn't running an agent. Check at request time.
- eve refuses to compile compaction for a model it cannot size, and it can only size AI
  Gateway IDs — which D8 guarantees we never use. Every agent needs
  `modelContextWindowTokens` (`agent.context_window`, falling back to 32k).
- `eve build` succeeding does **not** mean it typechecks. Run `npx tsc --noEmit` too.

### 5. Never vendor the neighbours (D4)
`ai-planner` (MCP + HTTP), `ai-worktree` (`awt` CLI), `file-sql` (MCP), ClickUp and Figma
(MCP, **read-only**) are used over their own interfaces. A change that makes any of them
impossible to run standalone is the wrong change.

They are **not** reachable as eve connections: `defineMcpClientConnection` requires an
HTTP url, and `aip serve` / file-sql speak MCP over stdio. Nothing generates
`agent/connections/` — M2-S8 picks the route that actually works.

### 6. Supervision talks HTTP by hand
`supervise/http.rs` is a small HTTP/1.1 client, not a dependency. Everything it talks to
is a process ai-team started on `127.0.0.1`, so a TLS stack and a connection pool would
be cost with no benefit — but **chunked framing has to be right**: eve streams NDJSON and
a chunk boundary lands mid-line constantly, so only whole lines are handed on.

One eve process per leased worktree means one `EveProcess`, and it is `kill_on_drop` —
leaking a Node server per crash is how a machine ends up full of them. Read both child
pipes concurrently: draining stdout first deadlocks the moment npm fills stderr.

### 7. Ingest eve's stream exactly once
`event.eve_event_id` is eve's `meta.id` under a partial unique index, and ingest uses
`INSERT OR IGNORE` — so a reconnect or a full rewind is free. Two traps that a real turn
exposed and fixtures did not: token and turn **counters** must only accumulate for rows
that were genuinely new, and `node_run.stream_cursor` is `from_index + batch.len()`, never
`cursor + batch.len()` (that is right for a resume and silently wrong for a rewind).
