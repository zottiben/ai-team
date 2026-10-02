# Built-in toolbox capability coverage

Checked against `ai-toolbox` revision
`90659e82f0d040315ff99cfbd765d8798eb3e085`, especially
`crates/ai-toolbox/src/cli.rs`, `cmd/{install,rules}.rs` and the core planners.
This is an implementation inventory, not a claim that R6 or desktop acceptance is
complete. The delivery plan remains in ai-planner.

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
| `skill --user` | Separately scoped user approval, not the project root picker | **Remaining R6**; no HOME write authority in project previews |
| `doctor --fix` | Findings and Preview repairs | Implemented; no-history classification conservatively calls differing content modified; repair preserves edited scripts |
| `bootstrap`, `init` | Recommendations start checked; Select recommended items, harness choices, optional missing AGENTS/CLAUDE scaffold | Implemented; no noninteractive approval bypass or overwrite of knowledge files |
| `rules <names>` | Searchable rule snippets, exact read-only text and Copy | Implemented; matches upstream print-only semantics, never appends unread rules |
| Templates / starter agents / background recipes | Read-only catalogue text and Copy; AGENTS/CLAUDE scaffold remains separate | Implemented for bundled assets; engine has no general template-to-arbitrary-path command, so no new write product is invented |
| `with-dotenv` | Explicit standalone .env launcher checkbox, also pulled in by relevant presets | Implemented; executable file preview, no .env read/write or secret entry |
| `migrate` layout | Preview layout migration moves hooks/helpers and Claude skills into canonical .agents, re-points harness configs, keeps complete trees/modes | Implemented safe adapter; conflicting bytes/types/modes refuse the whole preview; no root-harness-directory pruning |
| `migrate` Pi normalization/folding | Current adapter helper paths re-pointed; legacy .pi/mcp.json retained without importing servers, dropping fields or sharing dynamic headers | **Remaining R6** for explicitly chosen safe legacy import/normalization; current UI says this limitation, not full migration parity |
| `worktrees` | Read-only bounded Git worktree inventory and configuration differences | Implemented |
| `worktrees --sync` | Exact source/target convergence approval with both roots guarded | **Remaining R6**; scan is not consent to change linked worktrees |
| `base-charter [path]` | Charter text is browsable/copyable; global append needs separate user scope | **Remaining R6** for approved global/custom-target append; no project approval reaches HOME |
| `pi-init` | External prerequisite notice: Pi owns its MCP package installation | Manual prerequisite, not a filesystem plan. Upstream runs `pi install npm:pi-mcp-adapter` and changes global Pi state; ai-team does not silently execute it |
| `projects`, `projects scan --root` | ai-team project list and explicit Add/Attach, automatic read-only onboarding scan | Registration implemented; **remaining R6** for bounded discovery of unregistered projects under selected scan roots |
| `projects forget`, `projects prune` | ai-team-owned registration metadata only; never delete checkouts or standalone registry rows | **Remaining R6** for previewed pruning/forgetting and chat/history ownership checks |
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
  must preserve the current adapter's meaning and expose conflicts, not simply
  rename a file and claim migration success.
- Engine skills and templates can mention external tools/accounts or standalone
  toolbox commands. Their original text remains visible, not silently rewritten.
  Project setup does not start a model, server, package installer, shell hook or
  credential flow.

## Verification

Core: `crates/ai-team-core/tests/toolbox.rs` and toolbox/store unit tests.
HTTP: `crates/ai-team-ui/tests/server/toolbox.rs` (including authenticated catalogue
and saved approvals). UI: `ui/src/Toolbox.test.tsx` and `Projects.test.tsx`.
Browser and full non-live gates are recorded in the plan; live accounts, native
macOS acceptance and complete R6 coverage must not be inferred from these tests.
