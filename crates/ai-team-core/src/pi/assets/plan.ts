// What a seat may do to the plan through `bash` (rule 5).
//
// Part of the ai-team guard extension for Pi. A seat reaches ai-planner through its MCP
// server, whose tools are an allow-list: a maker reads the board and records what
// happened, and only the planner shapes it. `bash` answers to none of that, and the first
// real run had a frontend seat move its own PR to `in_review` with `aip slice set` before
// anything had checked it - the board said ready what ai-team had not yet judged. So a
// seat without the planning tools is held here to the same allow-list, spelled as `aip`
// subcommands.
//
// Like the rest of the guard this is not a security boundary: `bash` can spell anything.
// It stops the ordinary case - an agent following a charter written for a person working
// alone, which says to move the slice when the PR is open.

import { type Judgement, outsideQuotes } from "./irreversible.ts";

/** How the seat may treat the plan: `read`, `shape`, or unset where there is no plan. */
export const PLAN_ENV = "AI_TEAM_PLAN";

/** The `aip` subcommands a seat that only reads the plan may run: its MCP read tools. */
const READS = new Set([
  "current",
  "status",
  "ls",
  "find",
  "show",
  "logs",
  "resume",
  "log",
  "help",
  "slice ls",
  "slice show",
  "question ls",
  "decision ls",
  "gotcha ls",
  "handoff show",
  "handoff ls",
]);

/** Subcommands whose verb is the next word. */
const GROUPS = new Set(["slice", "question", "decision", "gotcha", "handoff"]);

/** Global options that take a value, so the value is not read as the subcommand. */
const VALUED = new Set(["-C", "--cwd", "-p", "--plan", "--db"]);

/** Words that run the command after them, rather than being it. */
const PREFIXES = new Set(["env", "command", "exec", "nohup", "time"]);

/** The words of each `aip` invocation in a command line, after `aip` itself. */
function invocations(command: string): string[][] {
  const found: string[][] = [];
  for (const segment of command.split(/&&|\|\||[;&|\n()`]|\$\(/)) {
    const words = segment.trim().split(/\s+/).filter((word) => word !== "");
    let at = 0;
    while (at < words.length && (PREFIXES.has(words[at]) || /^\w+=/.test(words[at]))) at += 1;
    const program = words[at];
    if (program === "aip" || program?.endsWith("/aip")) found.push(words.slice(at + 1));
  }
  return found;
}

/** The subcommand an invocation runs - `slice set`, `log` - or `undefined` for none. */
function subcommand(args: string[]): string | undefined {
  const words: string[] = [];
  for (let at = 0; at < args.length && words.length < 2; at += 1) {
    const word = args[at];
    if (word.startsWith("-")) {
      if (VALUED.has(word)) at += 1;
      continue;
    }
    words.push(word);
    if (!GROUPS.has(words[0])) break;
  }
  if (words.length === 0) return undefined;
  // A group with no verb prints its usage, like `--help`.
  if (GROUPS.has(words[0]) && words.length === 1) return undefined;
  return words.join(" ");
}

/**
 * May this command run, for a seat with this access to the plan?
 *
 * Refusals name what to do instead, as the other rules do: a bare "not allowed" sends a
 * model looking for another way to move the slice.
 */
export function judgePlan(command: string, access: string | undefined): Judgement {
  if (access !== "read") return { allowed: true };
  const normalised = outsideQuotes(command);
  for (const args of invocations(normalised)) {
    const sub = subcommand(args);
    if (sub === undefined || READS.has(sub)) continue;
    return {
      allowed: false,
      reason:
        `Refused: \`aip ${sub}\` changes the plan, and that is not this seat's to do. ` +
        "ai-team moves a slice as its work is checked, and the planner shapes the plan - " +
        "whatever a charter written for someone working alone says.\n\n" +
        'Record what happened with `aip log "..."`, and read the plan with `aip show` or ' +
        "`aip slice show`. If the plan is wrong, say so in your answer.",
    };
  }
  return { allowed: true };
}
