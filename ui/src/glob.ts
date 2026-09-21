/**
 * The zone/touches glob vocabulary, as the window sees it.
 *
 * **`crates/ai-team-core/src/util.rs` is the source of truth.** That matcher is what
 * dispatch routes a slice with, and this is a port of it so the map can light up the
 * paths a seat is working on without asking the server to walk the checkout on every
 * database tick - which is what expanding `crates/**` to real paths server-side would
 * cost, several times a second during a run.
 *
 * The port is deliberately literal, and `glob.test.ts` asserts the same cases
 * `util.rs`'s own tests do. If the two ever disagree the tests are where it shows, and
 * the Rust one wins: this decides what is *highlighted*, that decides where work
 * actually goes.
 *
 * The vocabulary is small on purpose: `*` stops at a separator, `**` does not, `?` is one
 * character that is not a separator, everything else is a literal.
 */

/** Does `path` match a single glob pattern? */
export function globMatches(pattern: string, path: string): boolean {
  return from([...pattern], 0, [...path], 0);
}

function from(p: string[], pi: number, t: string[], ti: number): boolean {
  while (pi < p.length) {
    const c = p[pi];

    if (c === "*") {
      const doubled = p[pi + 1] === "*";
      let rest = pi + (doubled ? 2 : 1);
      // `a/**/b` should also match `a/b`, so a `**` is allowed to swallow the separator
      // that follows it. This is the case that catches naive implementations.
      if (doubled && p[rest] === "/") rest += 1;

      if (rest >= p.length) {
        // A trailing `*` must not cross a separator; a trailing `**` may.
        return doubled || !t.slice(ti).includes("/");
      }
      for (let skip = ti; skip <= t.length; skip += 1) {
        if (!doubled && t.slice(ti, skip).includes("/")) break;
        if (from(p, rest, t, skip)) return true;
      }
      return false;
    }

    if (c === "?") {
      if (ti >= t.length || t[ti] === "/") return false;
    } else if (ti >= t.length || t[ti] !== c) {
      return false;
    }

    pi += 1;
    ti += 1;
  }
  return ti === t.length;
}

/**
 * Does any line of a zone claim this path?
 *
 * A zone is several patterns, one per line, and a `#` line is a comment. Reading a
 * comment as a pattern would hand a seat paths its author deliberately turned off.
 */
export function zoneMatches(zone: string, path: string): boolean {
  return patternsOf(zone).some((pattern) => globMatches(pattern, path));
}

/** The live patterns in a zone: trimmed, no blanks, no comments. */
export function patternsOf(zone: string): string[] {
  return zone
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"));
}

/** Does any of these patterns claim the path? What a slice's `touches` asks. */
export function anyMatches(patterns: string[], path: string): boolean {
  return patterns.some((pattern) => globMatches(pattern, path));
}
