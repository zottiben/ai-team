# Built-in toolbox capability coverage

Checked against `ai-toolbox` revision
`90659e82f0d040315ff99cfbd765d8798eb3e085`, especially
`crates/ai-toolbox/src/cli.rs`, `cmd/{install,rules}.rs` and the core planners.
This is the checked R6 implementation inventory, not desktop/live-account acceptance.
The delivery plan and gate evidence remain in ai-planner.

All implemented filesystem actions use the saved immutable preview, scoped root,
complete input fingerprints and single-use approval from R5. Nothing calls the
standalone toolbox CLI, discovers its registry, or writes through its installation.

| Upstream capability | ai-team surface / adaptation | Coverage |
| --- | --- | --- |
| `status`, `list`, `recommend` | Project setup scan, installed inventory, findings, recommendations; searchable bundled catalogue with complete source text | Implemented; catalogue enumeration tested against every shipped manifest entry |
| `hooks [names]` | Every bundled hook selectable for Claude/Codex/Pi; exact script and wiring preview | Implemented; Pi shell-hook wiring is explicitly unsupported by this engine |
| `mcp <names>` | Every preset selectable, shared config and selected harness conversion, credential/conversion warnings | Implemented; all presets tested across three harnesses, never connected during setup |
| `skill <names/groups>` | Full catalogue keys including grouped skills; project installation/update via exact preview | Implemented; unselected installations remain; explicit selection may replace local edits only after showing before/after |
| `skill --no-symlink` | Independent Claude copies; existing canonical link replaced as one frozen directory effect including unselected canonical skills | Implemented; no write through a symlink, existing real directories retain unrelated skills; stale tests cover both |
| `skill --user` | Projects → User-level toolbox setup; harness/skill selection and optional independent Claude copies | Implemented; separate immutable USER approval naming HOME/Pi agent directory, never authority inherited from a project |
| `doctor --fix` | Findings and Preview repairs | Implemented; no-history classification conservatively calls differing content modified; repair preserves edited scripts |
| `bootstrap`, `init` | Recommendations start checked; Select recommended items, harness choices, optional missing AGENTS/CLAUDE scaffold | Implemented; no noninteractive approval bypass or overwrite of knowledge files |
| `rules <names>` | Searchable rule snippets, exact read-only text and Copy | Implemented; matches upstream print-only semantics, never appends unread rules |
| Templates / starter agents / background recipes | Read-only catalogue text and Copy; AGENTS/CLAUDE scaffold remains separate | Implemented for bundled assets; engine has no general template-to-arbitrary-path command, so no new write product is invented |
| `with-dotenv` | Explicit standalone .env launcher checkbox, also pulled in by relevant presets | Implemented; executable file preview, no .env read/write or secret entry |
| `migrate` layout | Preview layout migration moves hooks/helpers and Claude skills into canonical .agents, re-points harness configs, keeps complete trees/modes | Implemented safe adapter; conflicting bytes/types/modes refuse the whole preview; no root-harness-directory pruning |
| `migrate` Pi normalization/folding | Preview legacy Pi import, separately from layout migration; merges legacy adapter settings into .pi/mcp-adapter.json and normalizes known transport/auth forms | Implemented safe adapter; keeps source/shared files and custom fields, refuses conflicting destination/shared transports or values, unknown legacy auth/transport and native-only Pi settings rather than dropping them |
| `worktrees` | Read-only bounded Git worktree inventory and configuration differences | Implemented |
| `worktrees --sync` | Worktree setup consistency → Preview convergence to one linked target | Implemented; both roots/membership guarded; active/retained targets refused and applying convergence reserves the target against new chat writers. Replaces matching files only after exact approval; keeps target-only paths and divergent link/copy layouts (reported, not silently converted) |
| `base-charter [path]` | User-level setup → Append base charter; optional explicit absolute custom destination | Implemented; resolved charter targets are named in USER authority, including a separately selected path outside HOME. Alias changes refuse apply; preserves existing text/markers and rejects non-text input |
| `pi-init` | External prerequisite notice: Pi owns its MCP package installation | Manual prerequisite, not a filesystem plan. Upstream runs `pi install npm:pi-mcp-adapter` and changes global Pi state; ai-team does not silently execute it |
| `projects`, `projects scan --root` | Projects → Discover and manage registrations; explicit scan/add and separately previewed remembered roots | Implemented; at most 16 roots, depth 6, 200 repositories/4096 entries, visible truncation and no child symlink traversal. Scan never registers or changes a checkout |
| `projects forget`, `projects prune` | Preview metadata-only archive, prune wholly missing projects, restore forgotten registrations | Implemented; revision/root/protected-work checks are transactional, and archived projects cannot admit new chat turns. Files, chats, runs, plans and receipts remain; Restore recovers the prior active/paused/done status, not a reopened replacement |
| `update [--check]` | Catalogue is packaged/versioned with ai-team and its existing updater, shown with revision/licences | No separate toolbox clone updater; no updates executed during onboarding |
| `ui` | Native ai-team project setup surface | Embedded replacement; no second server/window is launched |

## Boundaries grounded in the actual engine

- There is no general uninstall, catalogue CRUD or arbitrary template writer in the
  pinned CLI. Deselecting an item is **not** removal. Migration removes only legacy
  copies whose complete bytes and permissions survive canonically. Do not invent
  additional destructive actions to make the matrix look more complete.
- The pinned migration implementation can remove a differing legacy hook when the
  canonical filename exists, and can drop non-directory skill files. The adapter
  does its own bounded tree merge before reusing engine config conversion. Tests
  reproduce the unsafe upstream plan and require zero live writes on conflict.
- Whole-parent deletion is unsafe when only managed child inputs were inspected.
  Harness parent directories and unrelated config stay in place. Detector-only
  `.github` and `supabase` inputs carry existence markers, not recursive contents
  or write authority. Their unrelated symlinks/size cannot strand project setup.
- Legacy Pi folding can drop transport-conflicting overrides or top-level custom
  settings; copying current overrides into that planner is not compatibility.
  Launcher rewrites are restricted to server commands/arguments and dynamic header
  commands, leaving unrelated settings/strings intact. A separate explicit import
  preserves the current adapter's meaning and exposes conflicts, not simply
  renaming a file and claiming migration success. JSON-with-comments or unsupported
  native Pi shapes require manual reconciliation; the importer never guesses away
  data. Shared transports/dynamic headers are not migrated across harnesses.
- Engine skills and templates can mention external tools/accounts or standalone
  toolbox commands. Their original text remains visible, not silently rewritten.
  Project setup does not start a model, server, package installer, shell hook or
  credential flow.

## Verification

Core: `crates/ai-team-core/tests/{toolbox,toolbox_operations}.rs` and toolbox/store
unit tests. HTTP: `crates/ai-team-ui/tests/server/{toolbox,toolbox_operations}.rs`
(including auth, scope separation and reversible registration visibility).
UI: `ui/src/{Toolbox,ToolboxManagement,Projects}.test.tsx`.

Browser action-family walkthroughs, preserved filesystem/receipt evidence and the
actual installed adapter's read-only parser check are recorded in the plan. Apply
uses frozen bytes, not another catalogue read. Saved interrupted receipts are
inspectable across restart and cannot replay; global setup serialization blocks
further writes while a receipt remains `applying`. Reading an interrupted receipt
does not authorize retrying or clearing it. These checks do not imply
live server connectivity, OAuth, CI or native installed-app acceptance.
