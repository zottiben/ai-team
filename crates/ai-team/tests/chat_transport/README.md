# Actual Pi / chat-planner transport acceptance (opt-in)

This example is **not run by `cargo test`**. It uses an existing Pi executable and MCP
adapter but no live model, installed `awt`, operator profile, or keychain. Last exercised
with Pi **0.99.2** and `pi-mcp-adapter` **4.0.0** on macOS (previously 0.87.1/3.2.0).
It requires Git, Node and npm;
it does not install them or any package. Supply actual paths rather than these examples:

```sh
cargo build -p ai-team --example chat-transport
target/debug/examples/chat-transport \
  --pi /opt/homebrew/bin/pi \
  --adapter /Users/you/.pi/agent/npm/node_modules/pi-mcp-adapter/index.ts
```

## What really runs

The production solo/team drivers, process supervision, guards, scoped MCP configuration,
embedded `plan serve`, SQLite stores, real Git worktrees, npm gate, verifier and exact-tree
commit run unchanged. `current_exe()` hosts the same MCP entrypoint as the app. Only model
responses and the awt CLI transport are fixtures. The deterministic provider asks Pi to
execute real tools: it does not implement tool results or write plan rows itself.

The scenario proves:

- A positive unscoped discovery control actually launches the **fake** global MCP sentinel.
  The subsequent catalogue query and every chat role must not launch it. Adapters 3.2/4.0 use
  `mcp-adapter.json`; a sentinel in obsolete `mcp.json` alone would make this check vacuous.
- Pi's actual `auth check --no-refresh` for an unknown fixture provider loads no configured
  MCP extension or credential helper. This checks its distinct auth CLI, not a live account.
- A separate coordinator attempts an unavailable required context connection through the
  actual MCP gateway, then claims success. Structured failure evidence must still block it
  before any planner or lease; explicit cancellation leaves the other chats usable.
- Solo creates a plan/question over MCP, a human answers, and solo reuses its session.
- The same chat switches to coordinator/planner, pauses for explicit reviewed approval,
  then acquires one lease and runs a maker and independent reader.
- A real maker is stopped after tools have run; process draining preserves its lease and
  session. Explicit continuation adds an attempt on that same lease, branch and session.
- Scope overrides, outside-lease writes, publication and maker self-certification receive
  the **expected refusal**, not just any tool error. A reader's excluded write tool is absent.
- Gates and verifier certify a real committed tree with the approved parent. The lease is
  returned once; the solo checkout stays clean on its original HEAD. Old authority expires.
- Switching back preserves the original solo session, plan and cross-mode context. Another
  chat's plan stays unchanged. This is not concurrent multi-chat execution acceptance.

## Isolation and its limits

The host clears inherited environment before starting Tokio, uses fresh HOME/config/state/
sessions and Git configuration, and opts into only fixture-local models. Pi loads only the
fixture provider, the supplied adapter and generated guards. The provider registers both
at load time and `session_start`, with static selection metadata; an unbound native provider
must fail, never fall back to a live endpoint. No provider model text is authored remotely.

Decoy metered credentials must disappear before Pi and gates. A Node preload refuses and
records socket/fetch attempts; npm's own update notifier is explicitly disabled too. Fake
`aip`, `claude`, `codex`, `security` and `gh` executables record unexpected invocation. Git
has no remotes, and its system/global configuration is disabled. These are fixture controls
and tripwires, **not an OS network sandbox or a security boundary**. They do not authorize
running an arbitrary untrusted Pi executable/extension.

Core startup defers every chat MCP server until scoped CLI flags bind. Model catalogue
queries use an empty exclusive MCP config while retaining provider extensions. The positive
control caught eager adapter initialization reading global configuration before flag binding;
`--offline` alone did not prevent it. Do not weaken the sentinel or replace actual MCP with
configuration-only assertions to accommodate a runtime upgrade.

The 240-second timeout and task-abort guard bound failures; process supervision is still
responsible for draining subprocess groups. Evidence is separated by Pi PID. Successful
runs remove their temporary directory; failures retain its printed address for inspection.
No installed/global file is rewritten. Failed evidence can contain this fixture's synthetic
prompts/sessions and should be removed only after its processes are confirmed stopped.

The main build uses eight chat Pi starts and twelve expected refusals (including standalone `aip` through bash); the failed-context
scenario adds one coordinator. Auth metadata and discovery controls are counted separately.

This does **not** certify installed-awt hooks/pool/network semantics, subscriptions/OAuth,
native windows, Team HTTP/UI, external-source grounding or nested script-call error
attribution, concurrent chat execution, retained-work physical cleanup, or the complete
failed/close/recovery lifecycle. Team controls remain hidden.
