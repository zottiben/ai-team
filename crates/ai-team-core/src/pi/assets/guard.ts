// The ai-team guard: what a Pi seat may not do (D20).
//
// eve had no tools of its own that reached a worktree, so ai-team generated them and put
// the rules inside. Pi arrives with `bash`, `read`, `write` and `edit` already built, and
// they answer to nobody - which is the one thing the pivot gives away and this gives back.
//
// `tool_call` fires after `tool_execution_start` and before the tool runs, and it can
// refuse. That is the whole seam: three rules, applied in one place, ahead of the only four
// tools that can reach the filesystem, a remote or the plan.
//
// No rule is a security boundary, and saying so matters more than the code. A model
// determined to escape has `bash`, and `bash` can spell anything. The containment that
// actually holds is that the worktree is disposable, the branch is a draft, and nothing
// in the process is authenticated to publish. These stop the ordinary case: an agent
// finishing a task and helpfully pushing it, or resolving a relative path one directory
// too far up.

import { judge } from "./irreversible.ts";
import { holdOn } from "./lifeline.ts";
import { judgePlan, PLAN_ENV } from "./plan.ts";
import { isInside, worktreeRoot } from "./worktree.ts";
import { dirname, isAbsolute, resolve } from "node:path";
import { realpathSync } from "node:fs";

/** Tools whose path arguments must land inside the lease. */
const PATH_TOOLS: Record<string, readonly string[]> = {
  read: ["path"],
  write: ["path"],
  edit: ["path"],
};

/**
 * Resolve as much of a path as exists, then re-append the rest.
 *
 * A file `write` is about to create does not exist yet, so realpath throws on it. The
 * existing ancestor is what can be a symlink; the tail cannot be, because it is not
 * anything yet. Duplicated from worktree.ts rather than exported from it, because that
 * file is the audited D3 boundary and is deliberately not a utility library.
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
      if (parent === head) return absolute;
      tail.push(head.slice(parent.length + 1));
      head = parent;
    }
  }
}

/** Where a candidate path really lands, relative to the lease. */
export function outsideLease(candidate: string): { real: string; root: string } | undefined {
  const root = worktreeRoot();
  const absolute = isAbsolute(candidate) ? resolve(candidate) : resolve(root, candidate);
  const real = realpathOfNearestParent(absolute);
  return isInside(root, real) ? undefined : { real, root };
}

/**
 * Decide one tool call.
 *
 * Exported and pure so it can be tested without a Pi session - the hook below is four
 * lines of wiring around it, and wiring is not where this goes wrong.
 */
export function decide(
  toolName: string,
  input: Record<string, unknown>,
): { block: true; reason: string } | undefined {
  if (toolName === "bash") {
    const command = typeof input.command === "string" ? input.command : "";
    for (const verdict of [judge(command), judgePlan(command, process.env[PLAN_ENV])]) {
      if (!verdict.allowed) return { block: true, reason: verdict.reason };
    }
    return undefined;
  }

  const keys = PATH_TOOLS[toolName];
  if (keys === undefined) return undefined;

  for (const key of keys) {
    const value = input[key];
    if (typeof value !== "string" || value === "") continue;
    const escape = outsideLease(value);
    if (escape !== undefined) {
      return {
        block: true,
        reason:
          `Refused: ${value} resolves to ${escape.real}, which is outside the worktree ` +
          `leased to you (${escape.root}).\n\n` +
          "Everything this task needs is inside that directory. If you believe you need " +
          "something outside it, say so in your answer rather than reaching for it.",
      };
    }
  }
  return undefined;
}

export default function aiTeamGuard(pi: {
  on: (
    event: "tool_call",
    handler: (
      event: { toolName: string; input: Record<string, unknown> },
    ) => { block: true; reason: string } | undefined,
  ) => void;
}) {
  pi.on("tool_call", (event) => decide(event.toolName, event.input));
  // Here rather than at import, so loading this file to test `decide` watches nothing.
  holdOn();
}
