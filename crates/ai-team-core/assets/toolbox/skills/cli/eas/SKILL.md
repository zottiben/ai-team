---
name: eas-cli
description: Operate Expo EAS — builds, submits, OTA updates, credentials for React Native apps. Use for mobile build/release tasks. CRITICAL — production builds are BILLED.
---

# eas — Expo build / release recipes

**EAS production builds cost money.** Never trigger a production build or submit
without an explicit request; check the project's release rule first.

- **Build:** `eas build --platform ios|android --profile <profile>` — prefer
  `preview`/`development` profiles for testing; `production` only on explicit ask.
- **Submit:** `eas submit -p ios|android` — a release action; explicit ask only.
- **OTA:** `eas update --branch <branch>` for JS-only changes (no store review) —
  cheaper than a build, but still deliberate.
- **Status:** `eas build:list` · `eas build:view <id>`

Safety: smoke-test in a simulator first; commit + push `main` (free) and **stop**
before any build / submit / tag. Builds and submits are billed and user-facing.
