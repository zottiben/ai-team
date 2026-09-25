// Run with: node --test plan.test.ts
//
// On both CI legs, like the rest of the guard: what a seat may do to the plan through
// `bash`, which its ai-planner allow-list never sees.

import assert from "node:assert/strict";
import { test } from "node:test";

import { judgePlan } from "./plan.ts";

const refused = (command: string, access = "read") => {
  const verdict = judgePlan(command, access);
  assert.equal(verdict.allowed, false, `${command} should be refused`);
  return verdict.allowed === false ? verdict.reason : "";
};

const allowed = (command: string, access: string | undefined = "read") => {
  const verdict = judgePlan(command, access);
  assert.equal(verdict.allowed, true, `${command} should be allowed`);
};

test("a seat that reads the plan cannot move a slice or reshape the plan", () => {
  // The first real run: a frontend seat moved its own PR to in_review before anything had
  // checked it, so the board said ready what ai-team had not yet judged.
  for (const command of [
    "aip slice set PR1 in_review",
    "aip -C . -p review-tab slice set PR1 done",
    "aip --plan=review-tab slice edit PR1 --pr https://example.invalid/1",
    "cd ui && aip slice add 'PR3' --title x",
    "npm test && aip slice claim PR1",
    "aip slice release PR1",
    "aip question add 'should I?'",
    "aip question answer 4 yes",
    "aip decision add title why",
    "aip set done",
    "aip new 'another plan'",
    "aip delete review-tab",
    "aip section scope 'rewritten'",
    "aip handoff write --gate test=pass",
    "aip sync --fix",
    "aip serve --root .",
    "/opt/homebrew/bin/aip slice set PR1 done",
    "FOO=1 aip slice set PR1 done",
  ]) {
    refused(command);
  }
});

test("it can read the plan and record what happened", () => {
  for (const command of [
    "aip status",
    "aip show",
    "aip current --json",
    "aip resume",
    "aip ls",
    "aip find 'review tab'",
    "aip logs PR1",
    'aip log "T3 done: the review opens at its first commit" --slice PR1',
    "aip -p review-tab slice ls --json",
    "aip slice show PR1",
    "aip question ls",
    "aip decision ls",
    "aip gotcha ls",
    "aip handoff show",
    "aip --version",
    "aip --help",
    "aip slice --help",
    "aip help",
  ]) {
    allowed(command);
  }
});

test("only a command that runs aip is judged, not one that mentions it", () => {
  allowed("grep -rn 'aip slice set' docs/");
  allowed('git commit -m "aip slice set is refused now"');
  allowed("echo aip slice set PR1 done");
  allowed("cat notes/aip.md");
});

test("the planner, and a seat with no plan behind it, are not held to it", () => {
  allowed("aip slice set PR1 ready", "shape");
  allowed("aip slice add PR3 --title x", "shape");
  // A person driving one agent in a worktree: no run's plan to protect. Called directly,
  // because a default parameter would stand in for an `undefined` passed to the helper.
  assert.equal(judgePlan("aip slice set PR1 done", undefined).allowed, true);
});

test("a refusal says what the seat can do instead", () => {
  const reason = refused("aip slice set PR1 in_review");
  assert.match(reason, /aip log/);
  assert.match(reason, /ai-team/);
});
