# Vendored from typesafe-ai/skills

`SKILL.md` and `LICENSE` here are TypeSafe's official agent skill, copied
verbatim under MIT. **Never hand-edit them** - the change would be silently lost
on the next refresh, and a locally-patched copy can't be diffed against upstream.

| | |
|---|---|
| Source | <https://github.com/typesafe-ai/skills/tree/main/skills/typesafe-ai> |
| Pinned at | `65a39f3`, committed 2026-09-12 |
| Vendored on | 2026-09-23 |

Refresh from the clone root:

```bash
BASE=https://raw.githubusercontent.com/typesafe-ai/skills/main/skills/typesafe-ai
curl -fsSL "$BASE/SKILL.md" -o skills/typesafe-ai/SKILL.md
curl -fsSL "$BASE/LICENSE"  -o skills/typesafe-ai/LICENSE
```

Then update the pinned commit above:

```bash
curl -fsSL https://api.github.com/repos/typesafe-ai/skills/commits/main \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["sha"][:7], d["commit"]["committer"]["date"])'
```

Refresh when the integration starts disagreeing with the live docs: TypeSafe
names a stale skill as the cause of an agent inventing request or response
fields.

## Why vendored instead of `bunx skills add`

TypeSafe's own instructions are `bunx skills add typesafe-ai/skills --skill
typesafe-ai`, which still works if you want it outside this toolbox. It installs
**per-harness copies** though - `.claude/skills/`, `.pi/skills/`, one per agent -
which is the layout `ai-toolbox migrate` exists to undo. Vendoring keeps this
skill on the same path as every other one: a single folder in `.agents/skills/`
with `.claude/skills` symlinked at it, installed by `ai-toolbox skill typesafe-ai`
with no node runtime, no network, and no telemetry at install time.
