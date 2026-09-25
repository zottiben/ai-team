// Run with: node --test guard.test.ts
//
// The decision half of the guard, tested without a Pi session. `decide` is pure and the
// hook around it is four lines, which is the split that makes this testable at all.
//
// Runs on both CI legs (D12). macOS is where this behaves differently and is the platform
// the author cannot hand-test: /tmp is a symlink into /private, and APFS folds case.

import { strict as assert } from "node:assert";
import { mkdtempSync, mkdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, beforeEach, describe, test } from "node:test";

import { decide } from "./guard.ts";
import { resetWorktreeCache, WORKTREE_ENV } from "./worktree.ts";

const made: string[] = [];

function lease(): string {
  const dir = realpathSync.native(mkdtempSync(join(tmpdir(), "ai-team-guard-")));
  made.push(dir);
  process.env[WORKTREE_ENV] = dir;
  resetWorktreeCache();
  return dir;
}

after(() => {
  for (const dir of made) rmSync(dir, { recursive: true, force: true });
});

beforeEach(() => resetWorktreeCache());

describe("paths", () => {
  test("a write inside the lease is allowed", () => {
    lease();
    assert.equal(decide("write", { path: "src/lib.rs" }), undefined);
    assert.equal(decide("edit", { path: "./README.md" }), undefined);
    assert.equal(decide("read", { path: "Cargo.toml" }), undefined);
  });

  test("a relative path climbing out is refused", () => {
    lease();
    const verdict = decide("write", { path: "../escaped.txt" });
    assert.ok(verdict, "climbing out was allowed");
    assert.match(verdict.reason, /outside the worktree/);
  });

  test("an absolute path elsewhere is refused", () => {
    lease();
    assert.ok(decide("write", { path: "/etc/passwd" }));
    assert.ok(decide("read", { path: "/etc/shadow" }));
  });

  test("a symlink pointing out is refused, not followed", () => {
    // The reason a textual startsWith is not enough: the name is inside the lease and
    // the file is not.
    const root = lease();
    const outside = realpathSync.native(mkdtempSync(join(tmpdir(), "ai-team-outside-")));
    made.push(outside);
    writeFileSync(join(outside, "secret.txt"), "s");
    symlinkSync(outside, join(root, "link"));

    const verdict = decide("read", { path: "link/secret.txt" });
    assert.ok(verdict, "a symlinked escape was allowed");
    assert.match(verdict.reason, /outside the worktree/);
  });

  test("a file that does not exist yet is still judged", () => {
    // `write` creates files, so realpath throws on the target. Refusing everything that
    // does not exist would make the write tool useless; allowing it would make the guard
    // useless.
    const root = lease();
    mkdirSync(join(root, "src"), { recursive: true });
    assert.equal(decide("write", { path: "src/brand-new.rs" }), undefined);
    assert.ok(decide("write", { path: "../brand-new.rs" }));
  });

  test("the lease root itself is inside it", () => {
    const root = lease();
    assert.equal(decide("read", { path: root }), undefined);
  });

  test("a sibling directory sharing the prefix is not inside", () => {
    // `/tmp/lease-evil` must not pass because it starts with `/tmp/lease`. The separator
    // is what makes the prefix check a path check.
    const root = lease();
    const sibling = `${root}-evil`;
    mkdirSync(sibling, { recursive: true });
    made.push(sibling);
    assert.ok(decide("write", { path: join(sibling, "x.txt") }));
  });

  test("a tool with no path argument is not a path decision", () => {
    lease();
    assert.equal(decide("write", {}), undefined);
    assert.equal(decide("write", { path: "" }), undefined);
  });

  test("tools that cannot reach the filesystem are left alone", () => {
    lease();
    assert.equal(decide("web_search", { query: "/etc/passwd" }), undefined);
    assert.equal(decide("mcp", { server: "anything" }), undefined);
  });
});

describe("irreversible commands", () => {
  test("publishing is refused", () => {
    lease();
    for (const command of [
      "git push origin main",
      "npm publish",
      "cargo publish",
      "gh pr merge 4",
      "gh release create v1",
      "git tag v1.0.0",
    ]) {
      const verdict = decide("bash", { command });
      assert.ok(verdict, `${command} was allowed`);
      assert.match(verdict.reason, /^Refused:/);
    }
  });

  test("the refusal says what to do instead", () => {
    // A bare "not allowed" sends a model looking for a way around rather than on with
    // the work.
    lease();
    const verdict = decide("bash", { command: "git push" });
    assert.ok(verdict);
    assert.match(verdict.reason, /branch for review|human/i);
  });

  test("ordinary work is allowed", () => {
    lease();
    for (const command of [
      "cargo test",
      "git commit -m 'work'",
      "git status --porcelain",
      "npm run build",
      "git tag --list",
    ]) {
      assert.equal(decide("bash", { command }), undefined, command);
    }
  });

  test("searching for the words is not doing the thing", () => {
    // A guardrail that blocks grepping for a phrase is one people learn to work around,
    // which is worse than not having it.
    lease();
    assert.equal(decide("bash", { command: "grep -r 'git push' docs/" }), undefined);
  });

  test("a bash call with no command is not a decision", () => {
    lease();
    assert.equal(decide("bash", {}), undefined);
  });

  test("a seat that only reads the plan cannot move its slice through bash", () => {
    // Its ai-planner tools are an allow-list; `bash` is not, and a frontend seat moved its
    // own PR to in_review before ai-team had checked it.
    lease();
    process.env.AI_TEAM_PLAN = "read";
    try {
      const verdict = decide("bash", { command: "aip slice set PR1 in_review" });
      assert.ok(verdict, "the slice was moved");
      assert.match(verdict.reason, /^Refused:/);
      assert.equal(decide("bash", { command: "aip log 'T3 done' --slice PR1" }), undefined);
    } finally {
      delete process.env.AI_TEAM_PLAN;
    }
  });
});
