# ai-team

One prompt to an orchestrator, planned and built by a configurable team of AI agents,
working through your [ai-planner](https://github.com/zottiben/ai-planner) board. For one
engineer, on their own machine.

It runs entirely on the subscriptions you already pay for - no metered API keys, and a
per-machine profile so the work machine can deny a provider the personal machine allows.

> **Status: early.** `M0-S1` is in - the workspace, the gates, CI and the install and
> release skeleton. `ait ui` serves a page that reports its own version and nothing else
> yet. The store lands in `M0-S2`; the agents in M1.

## Install

```sh
curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh
```

Until the first release is tagged there is nothing to download, so the script falls back
to building from source and needs a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
ait doctor    # paths, machine profile, and whether the frontend was compiled in
ait ui        # open the window in a browser
```

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
