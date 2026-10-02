# Bun / Node monorepo — stack rules (starter snippet)

Common Bun / workspace footguns. Delete what doesn't apply.

- **Reproducible installs.** `bun install --frozen-lockfile` in CI; commit
  `bun.lock`. Don't mix package managers (no stray `package-lock.json`/`yarn.lock`).
- **Run tasks through the graph.** In a Bun/Turborepo monorepo use
  `turbo run <task> --filter=<pkg>` so dependency order and caching hold — don't
  `cd` into a package and run ad hoc.
- **ESM-first.** Bun is ESM by default: `import` not `require`, respect
  `"type": "module"` and explicit extensions in relative imports where required.
- **Prefer the project's scripts** (`bun run <script>`) over bare binaries — they
  encode the right flags/env. `bunx` for one-offs.
- **Node-compat isn't 100%.** Some Node APIs / native addons behave differently
  under Bun — verify a dependency actually runs on Bun before adopting it.
- **Add deps to the package that uses them**, not the workspace root, unless it's a
  shared devtool.
- **Gates:** strict TypeScript, ESLint flat config, and `test` all green before pushing.
