# ai-team — project knowledge

A local desktop platform where one prompt to an orchestrator is planned and built by a
configurable team of AI agents, working through the ai-planner board. For one engineer,
on their own machine.

**Stack:** Rust 1.98.1 (pinned), edition 2021, workspace shaped like `ai-planner`.
React 19 + Vite 8 + TypeScript, built to static assets and embedded in the binary.
Agents run as Vercel **eve** nodes (Node 24+).

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
Authored `bash`/`read`/`write`/`edit` tools act on a worktree leased with `awt get --lease`.
eve's sandbox is for genuinely untrusted execution only.

### 5. Never vendor the neighbours (D4)
`ai-planner` (MCP + HTTP), `ai-worktree` (`awt` CLI), `file-sql` (MCP), ClickUp and Figma
(MCP, **read-only**) are used over their own interfaces. A change that makes any of them
impossible to run standalone is the wrong change.
