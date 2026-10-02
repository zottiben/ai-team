# TypeScript — stack rules (starter snippet)

Common TS conventions / footguns. Delete what doesn't apply.

- **Strict base config.** Extend a strict `tsconfig` base: `strict: true`,
  `noUncheckedIndexedAccess: true`, `noImplicitOverride: true`,
  `verbatimModuleSyntax: true`. Ready-made set: **`@total-typescript/tsconfig`**
  (app/lib/monorepo × dom/no-dom × tsc/bundler) — see the TSConfig cheat sheet.
- **`ts-reset`** (one import of `reset.d.ts`) hardens the stdlib: `JSON.parse` /
  `res.json()` → `unknown`, `.filter(Boolean)` narrows, `.includes` on readonly
  arrays. Low-risk quality-of-life.
- **No `any`, no `@ts-ignore` to silence.** Use `unknown` + narrowing. If you must
  suppress, `@ts-expect-error` *with a reason* — it fails when the error is gone.
- **Parse at the boundary.** Validate external data (API responses, env, JSON)
  with Zod (or similar) and derive types from the schema — don't cast.
- **`type` vs `interface`:** `type` for unions/utilities, `interface` for
  extendable object shapes; be consistent with the project.
- **Discriminated unions over loose booleans** for state; exhaustive `switch` with
  a `never` default to catch missing cases at compile time.
- **Never hand-edit generated `.d.ts` / generated types** — regenerate from source.
- **Gates:** `tsc --noEmit` (or the project's typecheck) and the linter pass
  before pushing.
