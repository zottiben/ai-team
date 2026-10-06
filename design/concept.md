# ai-team — Concept

> Approved product direction · 2026-09-28
> Delivery plan: `codex-shaped-ai-team` in ai-planner.

## Product

A local, chat-first desktop coding application modeled on Codex's design, workflows
and desktop capabilities. Start a conversation in a project, let an agent work,
inspect the result, and continue steering without starting over.

Three differences define ai-team:

1. **Teams are optional.** Start with one agent; choose a configured team for work
   that benefits from it, including from an existing conversation.
2. **Every agent uses Pi.** Reuse the Pi harness and the working subscription/model
   integrations rather than introducing a second executor.
3. **Every chat has an Overview.** A dedicated command surface for that chat's
   plan, agents, approvals, execution controls and delivery.

Display the name **AI Team** and the existing teal logo; keep the stable app identifier,
CLI/package names and data locations. Codex is the interaction and visual reference,
not a reason to preserve the old dashboard-first product with different colours.

## Primary experience

### Projects and chats

The shell leads with projects and their chats, a clear New chat action, and a
spacious conversation canvas. A compact composer exposes project, local/worktree
context, model, permissions and Single agent / Team selection. Secondary tools
stay accessible without competing with the conversation.

A chat is a durable user workspace, **not** a run or a Pi session. It survives
settled turns, retries, completed runs and entry into team execution. Runs and
agent sessions are execution records beneath it. Returning to a chat restores its
history and working context; switching projects never silently redirects a reply.

Use restrained hierarchy, typography, spacing and semantic light/dark tokens in
line with the reference app. Inspect working, approval, error and review states,
not just its empty screen. Keyboard operation and reduced motion remain supported.

### One chat workspace

The conversation and composer stay in place. **Overview, Review, Board and Work**
open as contextual panels alongside that conversation, not a separate Chat/Overview
tab hierarchy. These panels command the selected chat's own work, never the project's
latest run. Switching panels preserves unsent messages and review/plan drafts.

Project tools contain only **Editor, Terminal, Source and Team**. Legacy run/review
links from Today remain separate history routes, not project tools or chat commands.

The chat's command panels contain:
- the plan, questions, approvals and evidence-backed progress;
- live agent activity, including the current tool/command and elapsed time;
- applicable stop, resume and retry controls with explicit target and effect;
- changes, review findings and delivery state.

For a single-agent chat, show the useful subset rather than an empty team
organisation chart. Team configuration describes who the agents are; live activity
describes what they are doing. Never turn token usage into a completion percentage.

### Solo and team work

New chats default to one Pi agent. Team execution is explicit and can be selected
before the first prompt or introduced later without losing the chat's history.
Planning and approval support the work; they are not mandatory ceremony for every
question or small edit.

Rust supervises execution, routes team work, manages worktrees and enforces policy.
An agent does not become the process supervisor by spawning its own sibling seats.
Retain trustworthy event streaming, interrupted-turn recovery, budget snapshots,
verification evidence and human-controlled publishing while changing the UX.

### Review and explicit delivery

A verified team draft is not a merged or published result. Review its recorded commit
and tree in the originating chat, then separately preview and approve local integration,
push and draft-PR creation against exact destinations. Local integration is clean-checkout
fast-forward only; divergent or dirty work is preserved rather than stashed or reset.
Manual policy remains a veto, and no delivery action implicitly performs the next one.

Retained work stays protected when ownership is uncertain. Inspection and findings do not
restart models or grant delivery approval. An interrupted delivery is inspected only after
its command groups drain; a missing remote result may still be delayed. An explicit,
reasoned acknowledgement can keep the inspected state without certifying success, changing
files/refs, returning leases or retrying publication.

### Project onboarding and tooling

Adding a project uses the built-in toolbox to scan its stack, harness setup,
installed tools, health and worktree consistency. Present recommendations and
configuration/repair choices as part of onboarding, not as a separate application.

Scanning is automatic; file changes are previewed and approved. Apply the approved
changes, reject stale previews, preserve local edits, and show any manual follow-up
such as missing credentials. Do not silently install software or rewrite global
harness configuration merely because a project was added.

The UI must cover the **full toolbox capability set**, not just its existing HTTP
routes: discovery/inventory, recommendations, scaffolding, hooks, MCP presets,
skills, rules/templates, layout migration, diagnosis/repair, worktree convergence,
and explicit user-level setup. Retain cross-harness support and canonical shared
files. No toolbox CLI port for agents is required. Catalogue assets must ship or
be managed by ai-team without requiring a separate toolbox installation.

## Built-in planning and standalone isolation

Planning becomes native ai-team functionality, reusing the planner engine where
practical. Humans work through the UI. UI, dispatch and agent interfaces share one
authoritative planning service for **ai-team-owned plans**.

The installed standalone ai-planner remains independent: other projects still use
it. Do not replace or modify `aip`, its database, global registrations or defaults.
Do not share its mutable store, auto-import its projects, or redirect clients into
ai-team. New planning commands use an ai-team namespace, not `aip`.

**Agent interface:** keep structured MCP tools hosted by ai-team (for example
`ait plan serve`), with a distinct server name and
seat-local configuration. Keep role tool allow-lists and scope calls to the chat's
project/plan. UI and Rust dispatch call the service directly, not through MCP or a
subprocess per operation. Additional CLI commands can serve diagnostics or
headless workflows without duplicating the entire old CLI surface.

Chat MCP discovery is explicit: generated planning/context servers and unrelated
checkout MCP servers are included; global MCP discovery is not inherited. This
prevents standalone planning from silently reappearing alongside the scoped tools.
It changes which servers a chat sees, not the operator's registrations or extensions.
Future toolbox onboarding can offer previewed, approved imports.

MCP is an interface, not a security boundary. Server-side validation and policy
still matter; an unrestricted shell cannot be contained by hiding a tool name.
`awt`, file-sql and read-only context integrations are not part of this absorption.

## Model access and setup — preserve what works

Keep the existing provider/model behavior, Pi catalogue integration, OAuth sign-in
and Settings setup. Preserve subscription-backed access, machine provider
restrictions and credential handling; this redesign is not an authentication
rewrite or an opening for metered API keys.

**Local inference is opt-in.** A fresh installation must not require an ailocal
server, model download or local runtime setup. Only use local models/fallbacks
when the operator has explicitly enabled and configured them. Respect existing
explicit model choices.

## Initial scope and later work

| Initial desktop scope | Direction |
| --- | --- |
| Projects and persistent chats | Codex-like navigation, history, search and organisation |
| Conversations | Live assistant/tool activity, steering and follow-ups, context/model controls |
| Execution | Local and isolated worktrees, setup, continuation and handoff |
| Review and delivery | Diffs, inline feedback, staging, commits and GitHub delivery |
| Working tools | Terminal, editor/file access, skills, MCP and notifications |
| Background work | Scheduled tasks, visible results and actionable attention |
| Planning and project setup | Built-in planner and full toolbox UI |
| Settings | Preserve working model/OAuth flows; make local setup optional |

Remote capabilities are revisited **after the first round of desktop testing and
usage**. Mobile, voice, computer-use/browser automation and general-purpose rich
document/image features are not requirements for this first delivery. Keep later
capabilities explicit in the parity inventory instead of implying they shipped.

Codex-like permission controls must describe actual enforcement. The current Pi
guard is not an OS sandbox; do not claim Codex-equivalent sandbox protection merely
because a selector looks the same. Design and test any stronger boundary explicitly.

## Rollout boundaries

macOS remains the primary desktop target; keep Linux checks working. Retain the
current application and its data during development, using isolated refactor/test
state. Other applications and repositories are not a test fixture.

The operator accepts a **fresh start for ai-team projects and history** when the
refactor is ready for usage/testing. Legacy project/run migration is not a delivery
requirement. Fresh project state does not mean resetting working OAuth credentials
or model configuration, deleting source checkouts, or touching standalone planner
projects. Any cutover/reset is deliberate, not a side effect of launching a build.

This direction replaces the former mandatory-team, dashboard-first and
never-integrate-planner/toolbox product constraints. It does not waive runtime
safety, truthful evidence, user control over writes or separation of unrelated data.
