# ai-team — Concept

> Status: draft · Author: zottiben · Last updated: 2026-09-23

## Elevator pitch

A desktop platform where you give one prompt to an orchestrator and a configured team of
AI agents plans it as pull requests, builds each in a worktree of its own, verifies its own
work, and brings you a diff to review — with the plan living on your ai-planner board the
whole time.

## Core fantasy

You stop being the for-loop. You describe intent, review work, and steer — and the thing
you steer is a team that gets better at your codebase every week, not a chat window that
forgets.

## Unique hook

Everyone else ships an agent. This ships an **org**, and it is honest about the two graphs
that implies:

- the **org graph** — a stable, configurable team of specialists with owned zones,
  persistent context and per-agent model routing. That is what ai-team adds.
- the **work graph** — the plan that splits, merges and gets cancelled as evidence arrives.
  That already exists as ai-planner, and ai-team works *through* it rather than beside it.

The second hook: it runs entirely on the subscriptions you already pay for. No metered API
keys, no balances to top up across several accounts, and a per-machine profile so the work
machine can deny a provider the personal machine allows.

## Target audience

One engineer — the author — running several projects at once and wanting to work at the
level of intent. Not a team product, not a hosted service, not a framework for other people
to extend. Everything runs locally on the machine it is installed on.

## Core loop

**Moment to moment:** prompt → the orchestrator plans onto the board, one user story per
pull request → each PR is built in its own worktree, its tasks taken in turn by the seats
that own them → a verifier on a different model checks the whole PR → a diff arrives in
your review queue → you comment → the responsible agent picks the comments up, in that
PR's worktree.

**Session:** open Today, see the one right next thing across every project, unblock it.

**Long term:** review feedback becomes project rules, so the team stops making the same
mistake twice and the accepted-change rate climbs.

## Pillars

1. **The org graph is ours, in Rust; the agent loop is eve's.** Routing, budgets, failure
   isolation, state and observability live in SQLite where we can see them. Every agent
   runs as an eve node - one runtime, no second executor. *Design test:* if a capability
   would only ever be visible inside a vendor's process, it belongs in Rust instead.

2. **Never build a graph where a loop would do.** A node earns a seat only if it needs a
   different model, a different tool surface, or is a read-only reviewer. *Design test:* if
   collapsing two nodes into one loses nothing, collapse them. Default team is six.

3. **The harness is the product.** Tool gateway, maker-≠-checker verification, compiled
   context, guardrails, observability, routing, feedback. *Design test:* a new feature must
   name which harness layer it strengthens, or it is decoration.

4. **Reuse, don't absorb.** ai-planner, ai-worktree, file-sql and Claude Code are used over
   their own interfaces and never vendored. *Design test:* if a change would make one of
   them impossible to run standalone, it is the wrong change.

5. **Evidence over vibes.** Completion is gated on the project's own checks, and the
   headline metric is accepted-change rate and cost per accepted change — never tokens.
   *Design test:* a page that reports activity rather than accepted outcomes is not done.

## Anti-pillars

- NOT a cloud service. Nothing is deployed; everything runs on this machine.
- NOT metered. If a provider can only be reached with a pay-per-token API key, it is out.
- NOT a framework with an extension API for other people.
- NOT a 34-agent roster. Roles are added when a real handoff demands one.
- NOT a replacement for ai-planner, awt, file-sql or skelly.
- NOT a chat app that happens to edit files.

## Scope & non-goals

- In scope: team configuration, orchestration, one worktree per pull request, verification,
  diff review with comments, Today, analytics, reminders, and an editing surface good enough
  to live in.
- Non-goals: multi-user, hosting, billing, mobile, and any model access that does not run
  through a provider already configured on this machine.

## MVP & scope tiers

- **MVP** (M0–M2): a prompt produces a plan on the board, two agents build slices in
  parallel worktrees, a verifier gates them, and the result is inspectable from the CLI.
- **v1** (M3–M5): the full window — Console, Board, Today, Review, Analytics, Reminders —
  plus the IDE surface (tree, editor, LSP, terminal, source control), installed by one curl
  and updatable from the GUI.
- **Later / maybe:** remote agents on other machines, a second human, ClickUp write-back
  beyond status, voice.

## Risks & open questions

- **The Claude-subscription bridge.** The AI SDK provider does not auto-bridge tools, so a
  Claude node's tools must be handed to the Claude Agent SDK as an in-process MCP server -
  the pattern pi-claude-subscription already proves. Verified working end to end. It is only
  available to us because ai-team generates the eve project and therefore owns both sides of
  the bridge (D7). This is the load-bearing bet; M1 exists to retire it before anything is
  built on top.
- **Durability cost, accepted.** Claude Code runs its own loop inside one eve step, so a
  crash mid-turn loses that turn. Pi has the same property today.
- **Cold-node prefix.** A Claude node's measured floor is ~64k tokens, but it is a cached
  prefix: the warm call read 61,416 cached and wrote 2,686. It costs rate limit on a cold
  node, not dollars. Favour long-lived nodes over short-lived ones.
- **IDE scope.** M4 roughly doubles the surface area. It is in v1, but it lands after the
  control plane is genuinely useful, so a slip there does not block daily use.
