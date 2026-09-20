// The worktree guard, tested on the platform that will actually run it.
//
// This is the D3 boundary: everything an agent can touch passes through resolveInside.
// It is also the piece most likely to be subtly wrong on macOS and right on Linux
// (D12), so it is a plain `node --test` file with no dependencies - CI runs it on both
// legs, which is the only way the author sees macOS behaviour at all.
//
//   node --test crates/ai-team-core/src/generate/assets/lib/worktree.test.ts

import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { afterEach, beforeEach, describe, test } from "node:test";

import { isInside, resolveInside, resetWorktreeCache, worktreeRoot, WORKTREE_ENV } from "./worktree.ts";

let base: string;
let root: string;
let outside: string;

beforeEach(() => {
  // mkdtemp returns /var/folders/... on macOS, which realpaths to /private/var/...,
  // and /tmp/... on Linux. That difference is exactly what this file exists to pin.
  base = mkdtempSync(join(tmpdir(), "ait-"));
  root = join(base, "worktree");
  outside = join(base, "elsewhere");
  mkdirSync(root, { recursive: true });
  mkdirSync(outside, { recursive: true });
  writeFileSync(join(outside, "secret.txt"), "not yours");

  process.env[WORKTREE_ENV] = root;
  resetWorktreeCache();
});

afterEach(() => {
  delete process.env[WORKTREE_ENV];
  resetWorktreeCache();
  // A test that leaves a temp directory behind on every run is a test that slowly
  // fills the machine it is meant to be protecting.
  rmSync(base, { recursive: true, force: true });
});

describe("worktreeRoot", () => {
  test("is realpath'd, which is what makes macOS work at all", () => {
    // On macOS the configured path and its realpath differ (/var vs /private/var). If
    // the root were not resolved, every candidate would look like an escape.
    assert.equal(worktreeRoot(), realpathSync.native(root));
  });

  test("refuses to guess when it is not set", () => {
    delete process.env[WORKTREE_ENV];
    resetWorktreeCache();
    assert.throws(() => worktreeRoot(), /AI_TEAM_WORKTREE is not set/);

    process.env[WORKTREE_ENV] = "   ";
    resetWorktreeCache();
    assert.throws(() => worktreeRoot(), /AI_TEAM_WORKTREE is not set/);
  });

  test("says so when it points at nothing", () => {
    process.env[WORKTREE_ENV] = join(root, "does-not-exist");
    resetWorktreeCache();
    assert.throws(() => worktreeRoot(), /does not exist/);
  });
});

describe("resolveInside", () => {
  test("accepts a relative path and returns it resolved", () => {
    mkdirSync(join(root, "src"), { recursive: true });
    writeFileSync(join(root, "src/lib.rs"), "fn main() {}");
    assert.equal(resolveInside("src/lib.rs"), join(worktreeRoot(), "src", "lib.rs"));
  });

  test("accepts the root itself", () => {
    assert.equal(resolveInside("."), worktreeRoot());
  });

  test("accepts a path that does not exist yet", () => {
    // write_file legitimately creates files. realpath throws on a missing path, so the
    // guard resolves the nearest existing ancestor and re-appends the rest.
    const target = resolveInside("src/brand/new/file.rs");
    assert.equal(target, join(worktreeRoot(), "src", "brand", "new", "file.rs"));
  });

  test("rejects a traversal out of the worktree", () => {
    assert.throws(() => resolveInside("../elsewhere/secret.txt"), /outside the leased worktree/);
    assert.throws(() => resolveInside("src/../../elsewhere"), /outside the leased worktree/);
  });

  test("rejects an absolute path outside the worktree", () => {
    assert.throws(() => resolveInside(join(outside, "secret.txt")), /outside the leased worktree/);
    assert.throws(() => resolveInside("/etc/passwd"), /outside the leased worktree/);
  });

  test("rejects a symlink that points out of the worktree", () => {
    // The one a textual startsWith check lets through: the path is spelled inside the
    // worktree and resolves outside it.
    symlinkSync(outside, join(root, "escape-hatch"));
    assert.throws(
      () => resolveInside("escape-hatch/secret.txt"),
      /outside the leased worktree/,
    );
  });

  test("rejects a symlinked file, not just a symlinked directory", () => {
    symlinkSync(join(outside, "secret.txt"), join(root, "innocent.txt"));
    assert.throws(() => resolveInside("innocent.txt"), /outside the leased worktree/);
  });

  test("allows a symlink that stays inside the worktree", () => {
    mkdirSync(join(root, "real"), { recursive: true });
    writeFileSync(join(root, "real/file.txt"), "mine");
    symlinkSync(join(root, "real"), join(root, "alias"));
    assert.equal(resolveInside("alias/file.txt"), join(worktreeRoot(), "real", "file.txt"));
  });

  test("a sibling directory sharing the root's prefix is not inside it", () => {
    // The classic off-by-a-separator: "/a/worktree-evil" starts with "/a/worktree".
    const sibling = `${root}-evil`;
    mkdirSync(sibling, { recursive: true });
    assert.throws(() => resolveInside(sibling), /outside the leased worktree/);
  });
});

describe("isInside", () => {
  test("treats the root as inside itself, with or without a trailing separator", () => {
    assert.ok(isInside("/a/b", "/a/b"));
    assert.ok(isInside(`/a/b${sep}`, "/a/b"));
    assert.ok(isInside("/a/b", "/a/b/c/d"));
  });

  test("does not confuse a prefix with a parent", () => {
    assert.ok(!isInside("/a/b", "/a/bc"));
    assert.ok(!isInside("/a/b", "/a"));
    assert.ok(!isInside("/a/b", "/other"));
  });

  test("follows the platform's own case rules", () => {
    // APFS is case-insensitive by default, so /Users/me/wt and /Users/me/WT are one
    // directory; on Linux they are two. Asserting the platform's real behaviour is the
    // point - a guard that is case-sensitive on macOS answers the wrong question.
    const caseInsensitive = process.platform === "darwin" || process.platform === "win32";
    assert.equal(isInside("/a/b", "/A/B/c"), caseInsensitive);
    assert.equal(isInside("/a/b", "/A/B"), caseInsensitive);
  });
});
