<!--
RULES.md — OPTIONAL. Only create this file if you have hard constraints that are
either (a) shared across multiple projects, or (b) meant to be read by humans /
CI as well as the model. Otherwise put your rules directly in AGENTS.md and
delete this template — an empty RULES.md is worse than none (that's how
frameworks rot: empty rule stubs while the real rules live elsewhere).

If you do use it, import it from AGENTS.md instead of duplicating:

    ## Rules
    @RULES.md
-->

# Rules — <project or org>

Hard constraints. Each is a "must" / "never", not a preference. State the rule,
why, and (if any) how it's enforced.

## Safety / data
- <e.g. Every new public table must enable Row Level Security. Enforced by CI test X.>

## Cost / irreversible actions
- <e.g. Never push a release tag without an explicit request — it triggers a billed build.>

## Code quality (non-negotiables)
- <e.g. No error-silencing (`as any`, empty catch, disabled lints) to make CI green.>

## Never touch
- <e.g. Generated files under `packages/types/generated/**` — regenerate from source.>
