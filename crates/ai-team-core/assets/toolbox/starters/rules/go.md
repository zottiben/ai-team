# Go — stack rules (starter snippet)

Common Go footguns. Delete what doesn't apply; add project specifics.

- **Wrap errors, don't swallow them.** `fmt.Errorf("doing X: %w", err)` and return
  early; check every error. Never `_ = err` to make it compile.
- **`context.Context` is the first arg and must propagate.** Thread the request
  ctx down; honour cancellation/deadlines; never store a ctx in a struct or reach
  for `context.Background()` deep in a call chain.
- **Goroutine + channel discipline.** Every goroutine needs a clear exit (ctx or a
  closed channel) — leaks are silent. Guard shared state; run tests with `-race`.
- **`defer` runs at function return, not block end.** Don't `defer` inside a hot
  loop (it accumulates). Always close rows/bodies/files.
- **`nil` interface gotcha.** A non-nil interface holding a nil pointer is not
  `nil`. Return concrete `error` values, not typed nils.
- **Slices share backing arrays.** `append` can mutate the original — copy when you
  need isolation; preallocate `make([]T, 0, n)` in hot paths.
- **pgx / database:** use a pool, pass ctx, `defer rows.Close()`, check
  `rows.Err()`. Parameterise (`$1`) — never string-concat SQL. Wrap multi-statement
  invariants in a transaction.
- **Table-driven tests** with `t.Run` subtests; `t.Parallel()` where safe;
  establish a baseline before claiming your change fixed or broke something.
- **Non-negotiable gates:** `gofmt`/`goimports`, `go vet`, and the project linter
  (e.g. golangci-lint) must pass. No unused/dead code.
