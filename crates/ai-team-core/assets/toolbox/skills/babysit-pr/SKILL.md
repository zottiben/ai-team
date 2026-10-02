---
name: babysit-pr
description: Watch a PR's CI to green — gather failing checks + review feedback, TRIAGE each item (fix / dismiss / escalate), fix the clear ones, lint, push, repeat up to a bounded number of rounds. Use to shepherd a PR through CI without blindly auto-fixing. Complements native PR-autofix tooling; this stays local and in-the-loop.
---

# babysit-pr — shepherd a PR to green (with triage)

Loop a PR to passing CI. The discipline that separates this from a churny bot:
**triage, don't blindly apply.**

1. **Watch:** `gh pr checks <pr> --watch`, then `gh run view <id> --log-failed`
   to collect failures. Also gather review comments (`gh pr view <pr> --comments`)
   and any bot/reviewer feedback.
2. **Triage each item** into **Fix** (clear + safe), **Dismiss** (wrong/irrelevant
   — note why), or **Escalate** (ambiguous or risky — ask the user). Act only on Fix.
3. **Fix** at root cause (no silencing). Run the project's checks locally first
   (the `pre-pr` skill).
4. **Push** and let CI re-run. Respect release rules — never tag or force-push.
5. **Repeat** up to a bounded number of rounds (default 3). Still red after that →
   stop and hand back a summary. Don't churn.

Bound it: cap the rounds and escalate rather than guess. Pair with a scheduled
runner for hands-off watching (e.g. Claude Code's `/loop 10m /babysit-pr <pr>`).
