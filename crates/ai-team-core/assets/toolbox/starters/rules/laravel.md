# Laravel — stack rules (starter snippet)

Common Laravel footguns. Delete what doesn't apply.

> TODO(you): record your work setup — Laravel version? API / Inertia / Livewire /
> Blade? Pest or PHPUnit? repo/service patterns? queue driver? deploy target?

- **Eloquent N+1.** Eager-load with `with()` instead of lazy-loading in loops;
  turn on `Model::preventLazyLoading()` in dev to catch it. Use `chunk`/`cursor`
  for large sets.
- **Mass assignment.** Set `$fillable` (or `$guarded`) deliberately; never pass
  unvalidated request input straight into `create`/`update`.
- **Validate at the edge.** Use Form Requests / `$request->validate()`; authorize
  with Policies/Gates, not ad-hoc `if` checks.
- **Migrations are the schema source of truth** — reversible, ordered; don't edit
  the DB by hand.
- **Config vs env.** Read config via `config()`, not `env()` outside config files
  (breaks under `config:cache`). Cache config + routes in production.
- **Queue the slow work** (mail, external calls); keep controllers thin; make jobs
  idempotent.
- **Query discipline in Blade too** — no queries inside view loops.
- **Gates:** the project's tests (Pest/PHPUnit), Pint, and static analysis
  (PHPStan/Larastan) pass before pushing.
