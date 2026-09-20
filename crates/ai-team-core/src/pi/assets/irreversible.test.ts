// Run with: node --test irreversible.test.ts
//
// On both CI legs, like the worktree guard: this decides what a node may do to the world
// outside its lease, and a regression here is one nobody notices until something has
// been published.

import assert from "node:assert/strict";
import { test } from "node:test";

import { judge } from "./irreversible.ts";

const refused = (command: string) => {
  const verdict = judge(command);
  assert.equal(verdict.allowed, false, `${command} should be refused`);
  return verdict.allowed === false ? verdict.reason : "";
};

const allowed = (command: string) => {
  const verdict = judge(command);
  assert.equal(verdict.allowed, true, `${command} should be allowed`);
};

test("publishing past the draft is refused", () => {
  for (const command of [
    "git push",
    "git push origin main",
    "git push --force-with-lease",
    "cd /tmp && git push",
    "npm publish",
    "pnpm publish --access public",
    "cargo publish",
    "gh pr merge 3 --squash",
    "gh release create v1.0.0",
    "git tag v1.0.0",
  ]) {
    refused(command);
  }
});

test("a refusal says what to do instead", () => {
  // A bare "not allowed" sends a model looking for a way around it.
  const reason = refused("git push origin main");
  assert.match(reason, /branch for review/);
  assert.match(reason, /a human/);
});

test("the ordinary work of building a slice is untouched", () => {
  for (const command of [
    "cargo test",
    "cargo fmt --all --check",
    "npm run build",
    "git status",
    "git diff HEAD",
    "git add -A",
    "git commit -m 'PR1: add median'",
    "git log --oneline -5",
    "git tag --list",
    "git checkout -b feature",
    "rm -rf target",
    "rm -rf ./node_modules",
    // A path that merely contains the word, or prose in a commit message.
    "grep -r 'git push' docs/",
    "echo 'do not git push' > NOTES.md",
  ]) {
    allowed(command);
  }
});

test("the filesystem root is not a target", () => {
  refused("rm -rf /");
  refused("rm -fr / ");
  // A path under the root is the node's own business; the worktree guard owns that.
  allowed("rm -rf /tmp/scratch");
  allowed("rm -rf target/debug");
});

test("matching is on command words, not substrings", () => {
  // `shutdown` inside a filename is not a request to stop the machine.
  allowed("cat src/shutdown_handler.rs");
  allowed("./scripts/reboot-tests.sh");
  refused("sudo shutdown -h now");
  refused("ls; reboot");
});
