# Vue 3 — stack rules (starter snippet)

Common Vue 3 (Composition API) footguns. Delete what doesn't apply.

> TODO(you): record your work setup — Nuxt or plain Vue? Composition vs Options
> API? Pinia? component lib (Vuetify/PrimeVue/…)? testing (Vitest/Cypress)?

- **Reactivity rules.** `ref()` needs `.value` in script (auto-unwrapped in
  templates); `reactive()` objects lose reactivity when destructured or spread —
  use `toRefs` / `storeToRefs`. Don't reassign a `reactive` object wholesale.
- **`computed` for derived state, `watch` for side effects** — don't trigger
  renders from a watcher you could express as a `computed`.
- **`v-for` needs a stable `:key`** (not the index for dynamic lists); never put
  `v-if` and `v-for` on the same element.
- **Props are one-way.** Don't mutate a prop — emit an event or use `v-model` with
  defined `update:` events. Declare props/emits explicitly with types.
- **`<script setup>`** is the default idiom (`defineProps`/`defineEmits`/
  `defineModel`); top-level bindings auto-expose.
- **State:** Pinia for shared state, not a global `reactive` blob; keep
  component-local state local.
- **Async & lifecycle:** create refs/watchers synchronously in `setup`; guard
  against updating an unmounted component.
- **Gates:** the project's typecheck (`vue-tsc`), ESLint, and tests pass before pushing.
