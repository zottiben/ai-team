# React — stack rules (starter snippet)

Common React (18/19) footguns. Delete what doesn't apply.

- **Effect deps are exhaustive.** List every value the effect reads; don't lie to
  the linter. If an effect runs too often, fix the deps (memoize, lift state) —
  don't strip the array.
- **Effects synchronise with the outside world**, they don't derive state. Compute
  derived values during render; prefer event handlers over effects for user
  actions. Avoid `useEffect(() => setState(...))`.
- **Stable keys.** Lists need stable, unique keys — never the array index for
  dynamic lists (it breaks reconciliation and local state).
- **Never mutate state.** Return new objects/arrays (`setX(prev => …)`); mutation
  breaks memoization and skips renders.
- **Server state ≠ client state.** Use TanStack Query for server data (caching,
  invalidation, loading/error) — don't hand-roll it in `useEffect`. Keep UI state
  in `useState`/`useReducer`, shared state in a store (e.g. Zustand).
- **Memoize deliberately.** `useMemo`/`useCallback`/`memo` only where render cost or
  referential identity matters — measure first, don't reflex-wrap.
- **Hooks are order-dependent.** Never call them conditionally or in a loop.
  Controlled inputs must stay controlled (don't flip between undefined and set).
- **React 19 shifts idioms** (`use()`, Actions, the compiler) — check the version
  before assuming legacy patterns.
