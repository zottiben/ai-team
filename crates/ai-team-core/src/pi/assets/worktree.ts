// The one place that decides what "inside the leased worktree" means (D3).
//
// Part of the ai-team guard extension for Pi. Not generated: this file ships as it is
// written, which is the point - the rule a node is held to should be readable and
// auditable rather than assembled.
//
// ai-team starts one Pi process per leased worktree (D10, D20) and sets the root in the
// environment, so it is a property of the process rather than of a request the model
// could influence.

import { realpath, realpathSync } from "node:fs";
import { dirname, isAbsolute, resolve, sep } from "node:path";

export const WORKTREE_ENV = "AI_TEAM_WORKTREE";

let cachedRoot: string | undefined;

/**
 * The worktree this process may touch.
 *
 * realpath'd, and that is load-bearing on macOS: /tmp and /var are symlinks into
 * /private, so an unresolved root never matches a resolved candidate and every path
 * would look like an escape.
 */
export function worktreeRoot(): string {
  if (cachedRoot !== undefined) return cachedRoot;

  const raw = process.env[WORKTREE_ENV];
  if (!raw || raw.trim() === "") {
    throw new Error(
      `${WORKTREE_ENV} is not set. ai-team starts one Pi process per leased worktree; ` +
        `this process has no worktree to act on.`,
    );
  }

  const absolute = resolve(raw.trim());
  try {
    cachedRoot = realpathSync.native(absolute);
  } catch (cause) {
    throw new Error(`${WORKTREE_ENV} points at ${absolute}, which does not exist`, { cause });
  }
  return cachedRoot;
}

/**
 * Resolve `candidate` and prove it is inside the worktree, or throw.
 *
 * Three things make this harder than a `startsWith`, and all three are macOS:
 *
 *  - symlinks: the candidate has to be realpath'd, or `wt/link-to-etc/passwd` passes a
 *    textual check while pointing anywhere at all;
 *  - a path that does not exist yet is legitimate (write_file creates files), and
 *    realpath throws on it - so the nearest existing ancestor is resolved instead and
 *    the remainder appended;
 *  - APFS is case-insensitive by default, so `/Users/me/wt` and `/Users/me/WT` are one
 *    directory and a case-sensitive comparison answers the wrong question.
 */
export function resolveInside(candidate: string): string {
  const root = worktreeRoot();
  const absolute = isAbsolute(candidate) ? resolve(candidate) : resolve(root, candidate);
  const real = realpathOfNearestParent(absolute);

  if (!isInside(root, real)) {
    throw new Error(
      `${candidate} resolves to ${real}, which is outside the leased worktree ${root}`,
    );
  }
  return real;
}

/** True when `target` is the root itself or sits beneath it. */
export function isInside(root: string, target: string): boolean {
  const base = stripTrailingSep(root);
  if (equalPaths(base, target)) return true;
  // `base` still ends in a separator when it is a filesystem root ("/"), in which case
  // everything absolute is beneath it - which is correct.
  const prefix = base.endsWith(sep) ? base : base + sep;
  return startsWithPath(target, prefix);
}

/**
 * Drop one trailing separator, but never reduce a filesystem root to the empty string:
 * "" as a prefix matches everything, which would turn the guard inside out.
 */
function stripTrailingSep(path: string): string {
  return path.length > 1 && path.endsWith(sep) ? path.slice(0, -1) : path;
}

/**
 * realpath as much of the path as exists, then re-append the rest.
 *
 * Resolving the existing ancestor is what catches a symlinked directory partway up;
 * the non-existent tail cannot be a symlink, because it is not anything yet.
 */
function realpathOfNearestParent(absolute: string): string {
  let head = absolute;
  const tail: string[] = [];

  for (;;) {
    try {
      const resolved = realpathSync.native(head);
      return tail.length === 0 ? resolved : resolve(resolved, ...tail.reverse());
    } catch {
      const parent = dirname(head);
      // Reached the filesystem root without finding anything that exists. Nothing more
      // to resolve, so the textual path is the best answer available.
      if (parent === head) return absolute;
      tail.push(head.slice(parent.length + 1));
      head = parent;
    }
  }
}

/**
 * Compare two paths the way the filesystem underneath would.
 *
 * Case-folded on macOS and Windows, exact on Linux. Deliberately keyed on the platform
 * rather than probing the mount: a case-sensitive APFS volume exists but is rare, and
 * folding there is merely stricter - it can refuse a path that would have been legal,
 * never permit one that should have been refused. The failure is safe in that
 * direction and not the other.
 */
const CASE_INSENSITIVE = process.platform === "darwin" || process.platform === "win32";

function equalPaths(a: string, b: string): boolean {
  return CASE_INSENSITIVE ? a.toLowerCase() === b.toLowerCase() : a === b;
}

function startsWithPath(value: string, prefix: string): boolean {
  return CASE_INSENSITIVE
    ? value.toLowerCase().startsWith(prefix.toLowerCase())
    : value.startsWith(prefix);
}

/** Async twin of realpathSync.native, for callers already in an async path. */
export async function realpathNative(path: string): Promise<string> {
  return await new Promise((ok, fail) =>
    realpath.native(path, (err, resolved) => (err ? fail(err) : ok(resolved))),
  );
}

/** For tests: forget the memoised root after changing the environment. */
export function resetWorktreeCache(): void {
  cachedRoot = undefined;
}
