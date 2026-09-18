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
| Configure the team | `ait team show`, `ait agents ls`, `ait agents edit <role> …` |
| Plan and dispatch | `ait run -p <project> "…"` (add `--plan-only` to stop after planning) |
| Drive one agent | `ait run -p <project> --worktree <dir> "…"` |
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

Fourteen decisions are recorded in the plan (`aip decision ls`). These eight are the ones
an agent will otherwise get wrong, so they are repeated here.

### 1. Subscription-backed models only (D8)
The entire point is to stop managing balances across several accounts, and the work machine
forbids some providers outright. Never introduce a metered API key path.

| Provider | Auth |
| --- | --- |
| Claude | Claude subscription, via the OAuthed Claude Code CLI |
| OpenAI | ChatGPT subscription, via eve's `chatgpt()` helper and `/login` |
| z.ai GLM | GLM Coding Plan (flat), openai-compatible at `https://api.z.ai/api/coding/paas/v4` |
| local | free, openai-compatible at the ailocal gateway on `127.0.0.1:8081` |

A machine profile (`~/.config/ai-team/machine.toml`) allows or denies each provider and is
enforced **at dispatch**, not only in the UI picker — a scheduled unattended run must not be
able to reach a denied provider. `ait init` creates the local-only default. Its `fallback`
array is a total ranking; a denied preference uses the first allowed, implemented provider
and records a Note event. Do not fall back on a transient health failure: that would silently
send work to a different account. Generated npm/eve processes must also remove inherited
metered credentials and cloud-Claude routing flags; not emitting `ANTHROPIC_API_KEY` is
insufficient if the operator's shell already exported it.

### 2. One runtime: eve (D7)
Every agent is an eve node. There is no second executor and no Pi. A Claude-subscription
agent is an eve node whose model is bridged:

```ts
model: claudeCode('sonnet', {
  mcpServers: { eve: createAiSdkMcpServer('eve', tools) },
  allowedTools: ['mcp__eve__bash', 'mcp__eve__read_file'],
  tools: [],              // disable Claude Code's own host tools
  settingSources: [],     // NOT undefined — undefined inherits the human's Claude config
})
```

This only works because we generate the eve project, so the generated `agent.ts` can import
the very tool modules it bridges. eve's *built-in* tools have no importable `execute` and
cannot be bridged — disable them on these nodes.

### 3. The database is the team; the eve project is generated (D2)
Never hand-edit anything under `.ai-team/agents/`. Change the team rows and regenerate.

`ait team` and `ait agents` are that edit surface; `--project` is optional because
`Store::project_at` resolves the checkout you are standing in. Two traps the schema sets:
an `INTEGER PRIMARY KEY` is a **reused** rowid, so an `old.id != new.id` check across a
delete silently compares one row to itself; and `project.team_id` has no FK (it forms a
creation-time cycle with `team.project_id`), so deleting a team has to clear it by hand.

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
HTTP url, and `aip serve` / file-sql speak MCP over stdio, so nothing generates
`agent/connections/`. The route that works is a **generated tool that shells out to the
neighbour's own CLI** — `agent/lib/plan.ts` drives `aip`, and Rust drives `awt` and `git`
from `neighbours/`. Adding a neighbour means adding a wrapper, never a dependency.

### 6. Supervision talks HTTP by hand
`supervise/http.rs` is a small HTTP/1.1 client, not a dependency. Everything it talks to
is a process ai-team started on `127.0.0.1`, so a TLS stack and a connection pool would
be cost with no benefit — but **chunked framing has to be right**: eve streams NDJSON and
a chunk boundary lands mid-line constantly, so only whole lines are handed on.

One eve process per leased worktree means one `EveProcess`. `npx` is only a wrapper, so
`kill_on_drop` on its direct child is insufficient — start it in its own Unix process group
and signal the **group**, or the Node server survives. Read both child pipes concurrently:
draining stdout first deadlocks the moment npm fills stderr.

### 7. The orchestrator plans; Rust leases and dispatches (D14)
`ait run` without `--worktree` runs the orchestrator seat to write an ai-planner plan,
then **Rust** reads the ready slices back, leases a worktree each with `awt`, and starts
one eve process per lease. An agent shelling out to `awt` and spawning sibling agents
would be the supervisor's job done with no budget or failure isolation around it.

ai-planner has **no dependency edges** — only `ord`, `status`, and a claim scoped to a
worktree. So dispatch means *ready, claimed, and the owning seat is idle*; a dependent
slice is held back by being left `blocked` rather than `ready`. Never add a deps table
here: that is plan structure, and copying it is what D4 forbids.

Routing is by **zone**. A slice must name the paths it touches (`plan_add_slice` requires
it, and writes them as a `Touches:` trailer on the scope); the seat whose zone owns them
builds it. A slice nobody owns is reported undone rather than given to somebody — guessing
is how two agents end up in one file.

**A leased worktree is borrowed.** `awt return` cleans and resets it, so a node's work is
committed to an `ai-team/<slice>` branch *before* the lease goes back. The worktrees are
git worktrees of one repo, so the branch survives; the worktree does not. A turn that
reports done but changed no file is recorded as **failed**, not done.

### 8. Ingest eve's stream exactly once
`event.eve_event_id` is eve's `meta.id` under a partial unique index, and ingest uses
`INSERT OR IGNORE` — so a reconnect or a full rewind is free. Three traps that real turns
exposed and fixtures did not: token and turn **counters** must only accumulate for rows
that were genuinely new; `node_run.stream_cursor` is `from_index + batch.len()`, never
`cursor + batch.len()` (that is right for a resume and silently wrong for a rewind); and AI
SDK v7's `inputTokens` is a total whose `cacheReadTokens` / `cacheWriteTokens` are subsets,
so subtract those subsets before storing the uncached input column.
