# Expo / React Native — stack rules (starter snippet)

Common Expo/RN footguns. Delete what doesn't apply.

- **EAS production builds are billed.** Don't trigger a release build (often gated
  on a `v*` tag or an EAS profile) without an explicit request — check the
  project's release rule first.
- **Prebuild / config-plugin model.** Native config lives in
  `app.json`/`app.config.*` + config plugins, not in hand-edited `ios/`/`android/`
  (those regenerate via `expo prebuild`). Adding a native module needs a rebuild,
  not just an install.
- **Dev client vs Expo Go.** Custom native modules require a dev client / prebuild;
  they won't run in Expo Go. Know which you target.
- **Platform differences are real.** Test iOS *and* Android; guard with
  `Platform.select`/`Platform.OS`; respect safe areas / the notch.
- **Reanimated runs on the UI thread.** Worklets can't freely touch JS state —
  follow the `runOnJS`/`runOnUI` rules and wrap the app in the required provider.
- **Big lists:** `FlatList`/`FlashList` with a stable `keyExtractor` (and
  `getItemLayout` where possible) — don't `.map()` large data sets.
- **Metro resolves differently** from web bundlers (no Node built-ins; watch
  monorepo symlinks). Clear the cache when resolution gets weird.
- **Smoke-test on a simulator/device** before calling a UI/behaviour change done —
  passing tests aren't enough for native.
