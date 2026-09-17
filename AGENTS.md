# ai-team — project knowledge

A local desktop platform where one prompt to an orchestrator is planned and built by a
configurable team of AI agents, working through the ai-planner board. For one engineer,
on their own machine.

**Stack (planned — nothing is built yet):** Rust workspace shaped like `ai-planner`
(`ai-team-core` SQLite store, `ai-team-ui` axum + SSE, `ai-team` CLI `ait`,
`ai-team-desktop` Tauri, kept out of `default-members`). React 19 + Vite frontend built to
static assets and embedded in the binary. Agents run as Vercel **eve** nodes (Node 24+).

**Layout** — the workspace does not exist yet. `M0-S1` creates it.
- `design/concept.md` — binding concept: pillars, anti-pillars, scope tiers
- `AGENTS.md` — this file

**Commands** — none yet. Fill this section in `M0-S1`, from the real manifests.

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

### 4. Agents edit leased worktrees, not eve's sandbox (D3)
Authored `bash`/`read`/`write`/`edit` tools act on a worktree leased with `awt get --lease`.
eve's sandbox is for genuinely untrusted execution only.

### 5. Never vendor the neighbours (D4)
`ai-planner` (MCP + HTTP), `ai-worktree` (`awt` CLI), `file-sql` (MCP), ClickUp and Figma
(MCP, **read-only**) are used over their own interfaces. A change that makes any of them
impossible to run standalone is the wrong change.
