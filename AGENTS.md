# ai-team — project knowledge

A local desktop platform where one prompt to an orchestrator is planned and built by a
configurable team of AI agents, working through the ai-planner board. For one engineer,
on their own machine.

**Stack:** Rust 1.98.1 (pinned), edition 2021, workspace shaped like `ai-planner`.
React 19 + Vite 8 + TypeScript, built to static assets and embedded in the binary.
Agents run as **Pi** processes (`pi`, with the operator's own extensions).

**Platforms — macOS is the target, Linux is the dev machine (D12).** ai-team is used daily
on macOS; it is built on Linux. Both must work, and where they disagree macOS wins. The
author cannot hand-test the primary platform, so CI's `macos-latest` leg is the real
verification — put platform-sensitive logic behind a test that runs on **both** legs
rather than checking it by hand. Three macOS facts bite:
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
| Bring in a ticket | `ait ingest <clickup-url> --brief-file <file>` |
| Plan and dispatch | `ait run -p <project> "…"` (add `--plan-only` to stop after planning) |
| Build what is ready | `ait run -p <project>` (no prompt; `--replan` plans again) |
| Drive one agent | `ait run -p <project> --worktree <dir> "…"` |
| The database | `ait db path`, `ait db open` (TablePlus), `ait db views` |
| What to work on now | `ait today` (ranked across every project) |
| Which pairing earns its seat | `ait stats --by agent\|model\|team\|project` |
| Schedule a run | `ait remind run "nightly" --in 2h`, and `ait daemon` to keep the clock |
| Update in place | `ait update` (`--check` to look only) |
| Icons | `cd crates/ai-team-desktop/icons && sh regenerate.sh` |

The root `cargo` commands skip `ai-team-desktop` on purpose — it pulls in Tauri and a
platform webview. CI compiles it separately so it cannot rot. On Linux it needs the
webview headers listed in `.github/workflows/ci.yml`.

## Plan and design

There is no `BUILD_PLAN.md` or `HANDOFF.md`. The plan is a row in ai-planner:

```sh
aip status            # where you are, what is next
aip show              # the whole plan: decisions, gotchas, slices
aip resume            # after a context clear
```

`design/concept.md` is the source of truth for scope. Build against it; when a decision
isn't covered, ask and record it with `aip decision add`.

## Hard rules

These thirteen are the rules an agent will otherwise get wrong. They were decided in the
first build plan, retired when `1f53679` became the baseline, so the `D` and `M` tags name
that plan's decisions and slices - not anything `aip decision ls` will find. This file is
now their only record.

### 1. Subscription-backed models only (D8)
The entire point is to stop managing balances across several accounts, and the work machine
forbids some providers outright. Never introduce a metered API key path.

| Provider | Auth |
| --- | --- |
| Claude | Claude subscription, via the OAuthed Claude Code CLI |
| OpenAI | ChatGPT subscription, via Pi's `openai-codex` provider |
| z.ai GLM | GLM Coding Plan (flat), openai-compatible at `https://api.z.ai/api/coding/paas/v4` |
| local | free, openai-compatible at the ailocal gateway on `127.0.0.1:8081` |

A machine profile (`~/.config/ai-team/machine.toml`) allows or denies each provider and is
enforced **at dispatch**, not only in the UI picker — a scheduled unattended run must not be
able to reach a denied provider. `ait init` creates the local-only default. Its `fallback`
array is a total ranking; a denied preference uses the first allowed, implemented provider
and records a Note event. Do not fall back on a transient health failure: that would silently
send work to a different account. Every child process must also remove inherited metered
credentials and cloud-Claude routing flags; not emitting `ANTHROPIC_API_KEY` is
insufficient if the operator's shell already exported it.

### 2. One runtime: Pi (D20)
Every agent is a `pi --mode json --print` child process, one per leased worktree. There
is no project to generate, no npm install, no build, no port and no token: stdout is the
stream, and a turn ends when the process does.

```rust
PiTurn { worktree, prompt, provider, model, thinking, exclude_tools, mcp_config,
         session_id, guard, instructions }
```

A seat is that struct. Changing its model is an argument, not a regeneration - which is
most of what D20 bought. Four things cost a build each to learn:

- **`claude-subscription` and `anthropic` sit side by side in Pi's catalogue** and only
  the first is the flat-rate subscription D8 protects. The mapping is a match on the
  `Provider` enum, never a string.
- **Instructions ride in the prompt, not `--append-system-prompt`.** That flag never
  reaches the model on `claude-subscription` - the Claude Code CLI owns its system
  prompt. A seat with no instructions is a coding assistant with a `bash` tool, and the
  orchestrator will cheerfully write the code instead of the plan.
- **A turn ends at `agent_settled`, not `agent_end`** - the latter carries `willRetry`,
  so stopping there records a retry's first attempt as the whole turn.
- **Pi echoes the prompt back as a `role: "user"` message.** Reading assistant text
  without checking the role lets a prompt containing `VERDICT: pass` verify itself.

Sessions resume by id, which is how a repair attempt keeps the conversation that produced
the work it is repairing. Within a PR a seat's session moves to each new row it opens
(`Store::continue_session`), **with its stream cursor**: events are keyed
`<session>:<index>`, and a row that restarted the index at 0 would collide with the rows
before it and have its events silently ignored. Replies waiting on the old row move too.
Not across providers - another account's model starts fresh, so every prompt stands alone.

### 3. The database is the team (D2)
A seat is rows, resolved into flags at dispatch. `~/.ai-team/seats/<project>/` holds only
the guard and one MCP config per seat, and is written from those rows - never edited.

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

### 4. The guard is what holds a seat to its lease (D3, D20)
Pi arrives with `bash`, `read`, `write` and `edit` already built, and they answer to
nobody. `pi/assets/guard.ts` is loaded with `--extension` and refuses, through
`pi.on("tool_call")`, anything that resolves outside `$AI_TEAM_WORKTREE` or that
publishes - `git push`, `npm`/`cargo publish`, `gh pr merge`, releases, tags.

**One Pi process at a time per leased worktree (D10),** with `cwd` on the lease and the
root named explicitly in the environment: a rule keyed on the working directory is a rule
a `cd` changes. The guard is installed *outside* the lease, because a guard a node can
edit is not a guard.

It is not a security boundary and the file says so. A model with `bash` can spell
anything. What contains a node is that the branch is a draft, nothing in the process is
authenticated to publish, and - except for a one-PR plan, which builds in the run's own
checkout on a branch made for it - the worktree is a lease nobody else works in.

`read_only` seats pass `--exclude-tools write,edit` and keep `bash`, so the guarantee is
"cannot edit source through its tools", not "cannot write a byte" - the verifier has to
be able to run the project's checks.

The guard is `node --test`ed on **both** CI legs, because its macOS behaviour is what the
author cannot see (D12): `/tmp` is a symlink, APFS folds case, and a path that does not
exist yet is still a path `write` is about to create.

### 5. Never vendor the neighbours (D4)
`ai-planner` (MCP + HTTP), `ai-worktree` (`awt` CLI), `file-sql` (MCP), ClickUp and Figma
(MCP, **read-only**) are used over their own interfaces. A change that makes any of them
impossible to run standalone is the wrong change.

Pi runs **stdio** MCP servers, so a neighbour that publishes one is reached through it
rather than through a wrapper: `aip serve --root <checkout>` is the planning seats' MCP
server, with `--root` deliberately the checkout and not the lease - a plan written inside
a copy is a plan nobody finds again. `awt` and `git` are still driven from `neighbours/`,
because they publish a CLI and not a server.

Which ai-planner tools a seat gets is an allow-list, the same mechanism as rule 10: a
maker reads the board and records notes, a planning seat shapes it, nobody gets
`delete_plan`. A maker that can add slices can give itself work.

### 6. Supervision is a pipe, not a protocol
A turn is a child process and its stdout is NDJSON, so there is no HTTP client, no
framing and no readiness wait. Two things carried over from the eve supervisor because
both were learned rather than designed:

- **Read both pipes concurrently.** Draining stdout first deadlocks the moment the child
  fills the stderr buffer, which on a failing turn is exactly when it happens.
- **Signal the process group.** Pi starts MCP servers and tool subprocesses; killing the
  direct child leaves them running.

Ingest is batched small and written as the turn runs, because `ait ui` and `ait run` are
separate processes and the window learns anything happened by watching `MAX(event.id)`
move. A turn that records nothing until it ends is a crew panel that says "starting" for
four minutes.

### 7. The orchestrator plans; Rust leases and dispatches (D14)
`ait run` without `--worktree` runs the orchestrator seat to write an ai-planner plan,
then **Rust** reads the ready slices back, leases a worktree for each PR with `awt`, and
hands it to its crew - one Pi process per task turn, one turn at a time in each lease. An
agent shelling out to `awt` and spawning sibling agents would be the supervisor's job done
with no budget or failure isolation around it.

The only dependency ai-planner records is a **stack**: a slice's `base_branch` naming
another slice's `branch` (PW9). Its MCP server cannot set one, so a planner seat writes
`Stacks on: PR1` in the scope and `stack.rs` writes the base through the CLI. **Every PR's
branch lives under its plan's slug**: right after planning, before anything is built, a
planner's own name `pr1-x` becomes `<plan>/pr1-x` (`stack::Names::Own`); later, names stand,
because work may be on them. A bare name is shared across plans, and since a branch with
commits is *continued*, a re-planned feature would silently build on the old plan's work.
A new plan's slug is never an existing branch, or git could not make `<slug>/...`. A child
builds once its parent is **built**
(`in_review`/`done`), not merged; the board is read again after every wave so it is
picked up in the same run. Never add a deps table here: that is plan structure, and
copying it is what D4 forbids.

**A run that plans starts on a fresh branch.** Its checkout is fetched and put on
`ai-team/run-<id>`, cut from `origin/<default>`; a checkout with uncommitted work, or with
another live run in it (`run.supervisor_pid`, checked with signal 0 so a crash does not hold
it forever), is refused. Nothing lands on the default branch unless the run was started with
`--on-default-branch`.

**ai-team never lets ai-planner infer the plan.** Asked without a name, it answers with
whichever plan the checkout has resolved to most - branch *or worktree path* - so a
checkout that planned before names last week's plan, and a planner reading "the board"
once deferred a slice in it. So Rust creates the run's plan itself (`Planner::create`,
titled by the orchestrator's `Plan:` line, based on the trunk so every slice copies that
base) before the planner's turn, and every seat's `aip serve` runs with
`AI_PLANNER_PLAN=<the run's plan>`. The orchestrator's grounding turn gets no planning
tools at all: there is no plan yet, and nothing to infer.

**A slice is one PR, built as its tasks (PW4).** The planner writes them into the scope,
one line each - `- T1 [backend] Title - Touches: paths` - and `tasks.rs` reads them back;
ai-planner has no level below a slice and is not to be changed for one, so the scope is
where they live and nothing copies them. The **owner is the authority** (PW5): an owner the
team cannot use (absent, switched off, read-only) leaves the PR unbuilt and says why, and a
path outside its zone is only noted - never silently rerouted. A slice with no task lines
is one piece of work for the seat whose **zone** owns its `Touches:` trailer, and one
nobody owns is reported undone - guessing is how two agents end up in one file.

**One writer at a time per checkout** (PW6). A PR's tasks run in order in its lease, each
committed as it finishes (`PR1 T2: title`), so the next starts from a known state. They
share one index, lockfiles and gates; parallelism comes from sibling PRs, never from seats
sharing a checkout. A seat is dispatched once per wave, and a PR holds its whole crew.

**One worktree per PR, not per agent** (PW3, PW10). A one-PR plan builds in the run's own
checkout; a larger one gives every PR an `awt` lease, and a built PR **keeps** it - with its
ai-planner claim - until it is merged or abandoned: it is where review comments are
worked on and what a stacked PR builds beside. Only a PR that stopped for good gives it
back; its branch keeps the commits. `git::put_on_branch` **continues** a branch that
already has work rather than `checkout -B`-resetting it to the base, and a base naming
the default branch starts from `origin/<default>`. A turn that reports done but changed
no file is recorded as **failed**, not done - unless it committed its work itself, which
the snapshot's `HEAD` shows.

**Review comments go back where the PR was built.** A finished seat's review, while its
PR is still in review and its worktree still on its branch, starts a follow-up run there
(`follow_up_at`): the seat takes them in its own conversation, and the whole PR is checked
again. It does not hold the checkout above it, so it runs while another run builds.

### 8. Done means this project's gates pass (M2-S9)
`gates.rs` **discovers** the checks from the repo's own manifests - cargo, and only the
npm scripts a `package.json` actually declares, including a nested `ui/`. Never hardcode
`cargo test`: a verifier that runs the wrong command reports green for a check that never
ran. No manifest means *no gates*, which is reported, not treated as a pass.

The verifier is asked the three things a green test run does not answer - **existence,
substantive, wired** - and answers `VERDICT: pass|reject`. Reading it **fails closed**:
anything that is not an explicit pass is a rejection.

**A PR is checked once it is whole** (PW7), against the commit it was built on - every task
is committed, so `git diff HEAD` would show nothing. Halfway through, the gates can fail
only because the next task has not been built. A rejection goes to the seat that can fix
it: a failing gate names its manifest and the zone owning `ui/package.json` gets it; a
verifier names one with `OWNER: <role>`; otherwise the last task's owner. It is recorded
on **that seat's** row, because that is the attempt that was rejected. Acceptance is a fact
about the PR, so every row stays `running` until the verdict and then takes it - a `done`
row is what analytics counts as accepted.

Two rules the loop broke once each. A model's answer is captured from the **stream**
(`PiEvent::assistant_message`), never by filtering `Note` rows back out of the event
table - ai-team's own dispatch notices live in that column and parsed as a verdict. And a
repair is a **retry**, so every attempt dispatches its own `node_run` row: reusing one
loses the earlier evidence and hands the next turn a finished session's cursor.

### 9. Guardrails are the run's, not the team's (M2-S10)
Every budget, cap and failure policy is **snapshotted onto `run` at creation** and read
back from there. A team edit must not loosen a run already going. `guardrails.rs` is the
only place that decides whether work may continue, checked *before* spending rather than
after - a limit noticed afterwards is an audit trail.

A run-wide budget stops the whole run; a node's own cap stops only that node. All three
`on_failure` policies leave the siblings alone - `escalate` parks the work and keeps its
claim, `abort_branch` releases it - and a blocked slice goes back to `blocked` with its
reason, never to `ready`, which would offer the next run the same slice with no memory of
why it failed.

**The gates run inside the PR's worktree and leave build output there.** ai-team writes `target/`,
`node_modules/`, `dist/` and `.output/` into `.git/info/exclude` for that lease,
and commits only what each turn changed: `git::snapshot` hashes every dirty path before a
turn and `changed_since` keeps the ones that differ after, so gate output nothing ignores
is never a seat's work. Two traps: `git status
--porcelain` writes `XY path`, so trimming the front eats an unstaged file's leading space
and every path starts a character late; and the gates are repo-wide, so a violation
anywhere rejects a node whose zone does not contain it.

Publishing is not a node's call - see rule 4. The guard refuses it, on both CI legs.

### 10. Context sources are read-only, by allow-list (D9, D15)
ClickUp and Figma reach a seat through its generated MCP config, scoped per seat — the
ticket reaches the seats that decide what the work is, the designs reach the seat whose
zone owns the UI, and nobody else pays the prompt for them.
`~/.config/ai-team/machine.toml` has a `[context]` block that is **fail-closed**:
unstated means denied.

Read-only is **enforced, not asked for**. Both servers expose writes — ClickUp
create/update/delete task, and Figma's `use_figma`, which creates, edits and deletes
despite reading like a read — so each server carries an `includeTools` list. Never
`excludeTools`: a wrong name on an allow-list costs a capability, a missed name on a
block-list hands over a write. It filters discovery as well as calls, which is the
property that matters — a tool the model can see is one it keeps trying.

The repository's own `.mcp.json` reaches every seat for free, because Pi's adapter merges
what it discovers from the lease with what ai-team supplies rather than replacing one
with the other (D19). Checked, not assumed: the other way round would have silently cost
a repo its own servers.

Ingested ticket text is **data, not instructions**. It is somebody else's writing arriving
in a prompt, and it is exactly the shape prompt injection takes.

### 11. The window is a view, and it polls (M3-S11)
`ait ui` and `ait run` are **separate processes** sharing one SQLite file, so there is no
in-process channel to subscribe to. `/api/events` polls `MAX(event.id)` and pushes an SSE
tick when it moves; identical ticks are suppressed. The tick is deliberately thin — it
says the database changed and the window re-reads whichever view it is showing, because
streaming rows would mean the server knowing what every surface renders.

`EventSource` cannot set a header, so the stream takes its token from the **query**. That
makes it the one route where an auth hole would go unnoticed, and it has its own test.

**What a checkout owns is one rule, in the store** (`WorkspaceScope`, PW1). A checkout a
run started in owns that run and every row of it, wherever its PRs were built. A PR's
worktree starts nothing: it owns the rows that built or checked the PR it holds there -
makers' turns and the verifier's, which is why a verifier row names its PR. Run lists, run
detail and events, reply, resume, deliver, reset, the crew and analytics all ask it; an
endpoint comparing `run.workspace_path` itself will list what it then refuses to open. The
rule compares paths in SQL, so every checkout path a run or node row holds is **stored
resolved** - `/var` and `/private/var` are different strings (D12). The sidebar's nesting is
derived too (`layout.rs`, from the building rows and the plan's bases), never stored.

Every surface resolves a semantic token (`--{category}-{role}-{state}`); the palette lives
only in `ui/src/tokens.css`. A raw colour in a component is a component that stays dark
when the window goes light — enforced by a test, because there is no browser on the
machine this is built on and nobody can simply look.

### 12. Ingest the stream exactly once
Pi does not label its events, so `event.eve_event_id` holds `<session>:<index>` under a
partial unique index and ingest uses `INSERT OR IGNORE` — so a replayed session start is
free, and a retry's evidence survives because a second session's event 0 is not the
first's. Three traps that real turns exposed and fixtures did not: token and turn
**counters** must only accumulate for rows that were genuinely new; `node_run.stream_cursor`
is `from_index + batch.len()`, never `cursor + batch.len()`; and `input` is a total whose
`cacheRead` / `cacheWrite` are subsets, so subtract them before storing the uncached
input column.

### 13. House rules are how every seat hears the repo (M3-S25, M8-S36)
`house.rs` reads what a checkout already carries — `AGENTS.md`, `CLAUDE.md`,
`CONVENTIONS.md`, `.cursor/rules`, `.github/copilot-instructions.md` — from the **lease**,
so a branch that changes the rules is judged by the rules it proposes. It is the only
channel a local, GLM or ChatGPT seat has, and the only one that carries `AGENTS.md` at all:
`settingSources: ['project']` loads a Claude seat's CLAUDE.md files, and AGENTS.md is not a
Claude convention.

Nested files are found too, because a repository puts its rules next to the code they
govern. They are **selected by what the task touches** — `ui/AGENTS.md` for a task
touching `ui/src/App.tsx` — matching a glob on its literal prefix, since a task line
writes `Touches: crates/**`. Sending every AGENTS.md in a monorepo is not context, it is
noise that crowds out the slice. Deepest-first under the 16k budget so the most specific
survives a cut; shallowest-first in the prompt so it reads as qualifying what came above.
A turn with no slice behind it gets all of them, because nothing narrows what it may edit.

ai-team never writes these files and never learns them (Q16). Read what is there, say
nothing when there is nothing.
