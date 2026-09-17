# ai-team

One prompt to an orchestrator, planned and built by a configurable team of AI agents,
working through your [ai-planner](https://github.com/zottiben/ai-planner) board. For one
engineer, on their own machine.

It runs entirely on the subscriptions you already pay for - no metered API keys, and a
per-machine profile so the work machine can deny a provider the personal machine allows.

> **Status: early.** `M0-S1` is in - the workspace, the gates, CI and the install and
> release skeleton. `ait ui` serves a page that reports its own version and nothing else
> yet. The store lands in `M0-S2`; the agents in M1.

Built for **macOS**, and developed on Linux — both are supported and both are gated in
CI on every push.

## Install

```sh
curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh
```

On macOS this installs the `ait` CLI and, when the release archive carries one,
`ai-team.app` into `/Applications`. The `.tar.gz` route carries no quarantine attribute,
so it launches without a Gatekeeper prompt; a `.dmg` downloaded in a browser does not.

Until the first release is tagged there is nothing to download, so the script falls back
to building from source and needs a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
ait init      # register this checkout and create a local-only machine profile
ait doctor    # paths, provider policy/reachability, and the embedded frontend
ait ui        # open the window in a browser
```

### Machine provider policy

Team rows are portable preferences; `~/.config/ai-team/machine.toml` is the permission
boundary on this machine. `ait init` creates a fail-closed default that allows only the
loopback ai-local gateway. Account-backed providers have to be enabled deliberately:

```toml
version = 1
fallback = ["claude", "openai", "zai", "local"]

[providers]
claude = true
openai = true
zai = false # forbidden on this work machine
local = true
```

When a seat's preferred provider is denied, the first allowed provider in `fallback`
whose integration exists is used with the registry's conservative default model. The
fallback is printed and recorded as an append-only run event. Reachability does **not**
change routing: a transient outage must not silently send work to another account.

The four allowed paths are deliberately specific:

- `claude` — the OAuthed Claude Code CLI; its eve bridge lands in M1-S6.
- `openai` — eve's `chatgpt()` subscription broker, never its metered `openai()` helper.
- `zai` — a GLM Coding Plan token in `AI_TEAM_ZAI_KEY`, sent only to the coding-plan URL.
- `local` — host, port, and gateway key read from ai-local's own config.

`ait doctor` reports each as `allowed`, `denied`, or `unreachable`. The profile is checked
again when every node is dispatched, including scheduled runs; hiding a denied provider
in the picker is not treated as enforcement.

## Develop

```sh
cargo test                                  # the workspace, minus the desktop shell
cargo clippy --all-targets -- -D warnings
cargo fmt --all

cd ui && npm ci && npm run build            # the frontend, embedded in the binary
```

The desktop shell is deliberately out of `default-members`: it pulls in Tauri and a
platform webview, and nobody changing the store should pay for either. Build it by name
with `cargo build -p ai-team-desktop`. On Linux that needs the webview headers - see the
package list in `.github/workflows/ci.yml`.

`ui/dist` is committed on purpose. The bundle is compiled into the binary so that
`cargo install` works with a Rust toolchain and nothing else, which means the built
output has to be in the repo. CI asserts it is current.

## Design

- `design/concept.md` - the binding concept: pillars, anti-pillars, scope tiers
- `AGENTS.md` - what an agent working in this repo needs to know
- The build plan is a row in ai-planner, not a file: `aip show -p ai-team`

## Licence

MIT
