# ai-team

A local Pi-powered coding workspace with persistent chats, built-in planning and a
command Overview for each conversation. For one engineer, on their own machine.
Optional team execution shares the same chat-first workflow, with explicit build approval.

It runs entirely on the subscriptions you already pay for - no metered API keys, and a
per-machine profile so the work machine can deny a provider the personal machine allows.

> **Chat-first release.** Solo/team chats, built-in planning, checkout tools and approved
> worktree setup are available. Legacy run history remains separate; it is not converted
> into chats. Operator acceptance testing continues independently of publication.

Built for **macOS**, and developed on Linux — both are supported and both are gated in
CI on every push. Releases include macOS universal and Linux x86_64/aarch64 builds.
Windows is not a supported release platform.

## Install

```sh
curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh
```

On macOS this installs the `ait` CLI and, when the release archive carries one,
`ai-team.app` into `/Applications`. The `.tar.gz` route carries no quarantine attribute,
so it launches without a Gatekeeper prompt; a `.dmg` downloaded in a browser does not.

**Upgrading from v0.7.5 or earlier:** finish active work, save unsaved edits, quit
AI Team and rerun the installer above once. Those versions do not have the integrated
updater, and their `ait update` replaces only the CLI. A failed release download stops
rather than silently building only the CLI. Explicit `--from-source` builds require a
Rust toolchain ([rustup.rs](https://rustup.rs)) and do not install the desktop app.
A desktop damaged by the legacy updater now explains this recovery path; merely opening
it never downloads, replaces programs or restarts the app.

### Updates

The integrated updater checks on opening, every 15 minutes while visible and when
returning after a pause. **Review update** (also in Settings) lists the exact programs,
versions and destinations before **Update listed programs**. `ait update` uses the same
service; `ait update --check` only looks. On macOS it updates the installed CLI and app
together, or just the app if no CLI is installed. It does not create a shadow CLI or
overwrite package-managed binaries. If multiple distinct CLIs are found, the desktop
refuses to guess; run the update command from the CLI you intend to use. Linux desktop
packages are updated through their AppImage/.deb distribution, not replaced with the CLI.

Finish agents and close app terminals/sign-in sessions before installing. Programs are
checksum-verified and staged before replacement; macOS native signatures are checked
before executing the downloaded CLI. These may be ad-hoc signatures: integrity checking
is not a notarization or publisher-certificate guarantee. Owned databases, configuration,
OAuth files and previous programs are backed up privately under `~/.ai-team/updates/`.
Backups remain until you remove them. No agent is stopped, no background sudo is requested,
and no app is restarted automatically. **Restart AI Team** after installation; reloading
a browser tab does not load a new server binary.

An interrupted swap retains its receipt and blocks new writes. **Inspect interrupted
update** or `ait update --inspect` checks the recorded bytes without downloading,
replaying installation or restoring data. Mixed installations remain fenced with their
backups for recovery. An updater also refuses to migrate a mismatched data schema: start
the matching app/`ait ui` build normally first, or use the paired bootstrap above.

```sh
ait init      # register this checkout; choose a provider in Settings (local is optional)
ait doctor    # paths, provider policy/reachability, and the embedded frontend
ait ui        # open the window in a browser
```

### Chats and built-in planning

The UI starts with persistent Pi chats and chat-local **Overview, Review, Board and Work**
panels. **Show archived chats** in the sidebar reveals archived conversations; **Restore
chat** preserves history and starts no work. Agents have no model-turn cap, in either
solo or team mode; configured token/time budgets and Stop remain separate controls.
Ordinary conversations need no plan. Optional team work requires explicit approval
of the exact ready slices; planning status and answers do not execute work or prove verification.

**Plans** groups plans by project with editable Board/Plan views. Archived-chat plans are
hidden by default and can be shown read-only. **Today** shows chat-owned activity and exact
chat links, not legacy runs. Editor and Terminal stay bound to the chat's checkout.

Existing checkout selection never resets files. Explicit new-worktree setup uses external
AWT's normal pool reuse/reset and hooks, then refreshes detected locked dependencies before
readiness. Failure retains the checkout and lease; inspection never replays acquisition.

Planning uses a pinned planner engine, not the installed `aip`. Its data is beside the
explicit team database as `<database>.planning.sqlite`; keep both together when backing
up state. Chat-scoped MCP is served by the running ai-team binary itself. It accepts
only the active chat/attempt and its allowed operations. Standalone planner data,
configuration and global registrations are not changed. Import is an explicit, reviewed
snapshot into AI Team-owned storage, never automatic or synchronized.

Chat MCP uses generated planning/context definitions and unrelated checkout servers;
it intentionally does not inherit global MCP discovery. Provider/OAuth settings and
operator Pi extensions are preserved. The legacy team CLI below remains separate from
chat execution. Commit, build verification, push, PR and merge are distinct authorities.

### Legacy team CLI

`ait init` seeds six seats - an orchestrator, a planner, two makers and two checkers.
The team is rows in the database - a seat is a set of flags on a Pi invocation, so
changing one takes effect on the next turn and this is where it is done:

```sh
ait team show                                  # guardrails and every seat
ait agents ls
ait agents edit backend --model glm-4.6 --provider zai --reasoning high
ait agents add --role release-manager --purpose 'Cuts releases.' --zone 'CHANGELOG.md'
ait agents tools backend --deny bash --note 'the verifier runs the gates'
ait team clone --from widget-service --to gadget-service
```

`--project` is optional: the checkout you are standing in decides. A seat owns paths (its
zone), and the seat that owns a path is the one the orchestrator dispatches it to. A
`--read-only` seat is generated with no editing tools at all rather than merely being
asked not to write.

### One prompt, several agents

```sh
ait run -p widget "Add a subtract function with a test, and show it in the UI."
```

The orchestrator turns the prompt into an [ai-planner](https://github.com/zottiben/ai-planner)
plan. ai-team then gives every ready slice its own worktree leased from
[ai-worktree](https://github.com/zottiben/ai-worktree), hands it to the seat whose zone
owns the paths it touches, and builds as many at once as the team's parallel width allows.
Each node's work is committed to an `ai-team/<slice>` branch before its worktree goes back
to the pool, and the slice moves to `in_review` pointing at that branch.

Run it again and it picks up whatever the board still has ready rather than planning the
same work twice - `ait run -p widget` with no prompt means exactly that, and `--replan`
plans again anyway.

Both tools are used over their own command line and neither is vendored, so `aip` and
`awt` keep working on their own. `ait doctor` says whether they are installed;
`--plan-only` stops after the plan, and `--worktree <dir>` runs a single turn instead.

### Tickets and designs

ClickUp and Figma come in as **read-only** context (D9), and only where they are useful:
the ticket reaches the seats that decide what the work is, the designs reach the seat
whose zone owns the UI.

```sh
ait ingest https://app.clickup.com/9014/t/86abc123 --brief-file ticket.md
```

That records the ticket as a project and pulls any Figma links out of the brief. ai-team
holds no credentials of its own - the seats read the ticket through their own connections.
Both are off until `~/.config/ai-team/machine.toml` says otherwise, and read-only is
enforced by an allow-list of tool names rather than by asking: both servers can create and
delete, and none of those tools is reachable.

### Machine provider policy

Team rows are portable preferences; `~/.config/ai-team/machine.toml` is the permission
boundary on this machine. `ait init` creates a fail-closed default with every provider
opt-in, including the loopback ai-local gateway. Enable only the providers you want:

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

- `claude` — the OAuthed Claude Code CLI, through Pi's `claude-subscription` provider.
- `openai` — the ChatGPT subscription, through Pi's `openai-codex` provider, never a metered key.
- `zai` — a GLM Coding Plan token in `AI_TEAM_ZAI_KEY`, sent only to the coding-plan URL.
- `local` — host, port, and gateway key read from ai-local's own config.

The supervisor removes inherited metered model keys and Claude cloud-routing switches
before Pi starts; an exported shell variable cannot silently change one of these paths.

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
- The build plan is a row in ai-planner, not a file: `aip show`

## Licence

MIT
