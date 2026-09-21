import { describe, expect, it } from "vitest";

import { activityOf, isBusy } from "./activity";
import type { BoardSlice, Member, RepoMap } from "./api";

/**
 * A small checkout:
 *
 *   0 (root) ─ 1 crates ─ 2 crates/core ─ 3 crates/core/lib.rs
 *            └ 4 ui ─ 5 ui/App.tsx
 *            └ 6 README.md
 */
function map(): RepoMap {
  const node = (path: string, name: string, dir: boolean, depth: number, owner: string | null) => ({
    path,
    name,
    dir,
    depth,
    weight: 1,
    owner,
  });
  return {
    root: "widget",
    files: 3,
    unowned: 1,
    truncated: false,
    nodes: [
      node("", "widget", true, 0, "backend"),
      node("crates", "crates", true, 1, "backend"),
      node("crates/core", "core", true, 2, "backend"),
      node("crates/core/lib.rs", "lib.rs", false, 3, "backend"),
      node("ui", "ui", true, 1, "frontend"),
      node("ui/App.tsx", "App.tsx", false, 2, "frontend"),
      node("README.md", "README.md", false, 1, null),
    ],
    edges: [
      { from: 0, to: 1 },
      { from: 1, to: 2 },
      { from: 2, to: 3 },
      { from: 0, to: 4 },
      { from: 4, to: 5 },
      { from: 0, to: 6 },
    ],
    zones: [
      { role: "backend", name: "Backend", zone: "crates/**", owns: 1 },
      { role: "frontend", name: "Frontend", zone: "ui/**", owns: 1 },
    ],
  };
}

function member(over: Partial<Member> = {}): Member {
  return {
    agent_id: 1,
    role: "backend",
    name: "Backend",
    provider: "local",
    model: "auto",
    read_only: false,
    zone: "crates/**",
    doing: "idle",
    node_run_id: null,
    run_id: null,
    slice_key: null,
    branch: null,
    attempt: 0,
    blocked_reason: null,
    last_said: null,
    reachable: false,
    turns: 0,
    tokens_in: 0,
    tokens_out: 0,
    ...over,
  };
}

function slice(key: string, touches: string[]): BoardSlice {
  return {
    key,
    title: key,
    status: "active",
    ord: 1,
    scope_md: null,
    demo_md: null,
    claimed_by: null,
    owner: null,
    touches,
  };
}

const pathsOf = (map_: RepoMap, indices: Iterable<number>) =>
  [...indices].map((at) => map_.nodes[at]?.path).sort();

describe("what the map shows as moving", () => {
  it("is completely still when nobody is mid-turn", () => {
    // The half of the design that makes the other half mean anything: a quiet repository
    // has to look quiet, or "busy" stops carrying information.
    const activity = activityOf(map(), [member(), member({ doing: "parked" })], []);

    expect(activity.hot.size).toBe(0);
    expect(activity.warm.size).toBe(0);
    expect(activity.live.size).toBe(0);
    expect(isBusy(activity)).toBe(false);
  });

  it("lights the paths a live slice says it touches", () => {
    const m = map();
    const activity = activityOf(
      m,
      [member({ doing: "working", slice_key: "M1-S1" })],
      [slice("M1-S1", ["crates/**"])],
    );

    expect(pathsOf(m, activity.hot.keys())).toEqual([
      "crates/core",
      "crates/core/lib.rs",
    ]);
    expect(activity.hot.get(3)).toBe("backend");
    expect(isBusy(activity)).toBe(true);
  });

  it("never lights the root, which would say everything is being worked on", () => {
    const m = map();
    const activity = activityOf(
      m,
      [member({ doing: "working", slice_key: "M1-S1" })],
      [slice("M1-S1", ["**"])],
    );
    expect(activity.hot.has(0)).toBe(false);
    expect(activity.hot.size).toBeGreaterThan(0);
  });

  it("draws the route from the root down to what is hot", () => {
    // The lines that carry the work: root → crates → crates/core → lib.rs.
    const m = map();
    const activity = activityOf(
      m,
      [member({ doing: "working", slice_key: "M1-S1" })],
      [slice("M1-S1", ["crates/core/lib.rs"])],
    );

    expect([...activity.live.keys()].sort()).toEqual([0, 1, 2]);
    // Coloured by whoever is doing the work, so two agents read as two things.
    expect(activity.live.get(0)).toBe("backend");
    // And nothing on a branch with nothing happening on it.
    expect(activity.live.has(3)).toBe(false);
    expect(activity.live.has(5)).toBe(false);
  });

  it("falls back to a seat's own territory when no slice narrows it", () => {
    // A turn driven straight at a worktree has no slice behind it. Claiming to know which
    // file it is editing would be a guess; claiming it is somewhere in its zone is not.
    const m = map();
    const activity = activityOf(m, [member({ doing: "working", slice_key: null })], []);

    expect(activity.hot.size).toBe(0);
    expect(pathsOf(m, activity.warm.keys())).toEqual([
      "crates",
      "crates/core",
      "crates/core/lib.rs",
    ]);
    // Warm is a statement about territory, not a route, so nothing flows.
    expect(activity.live.size).toBe(0);
    expect(isBusy(activity)).toBe(true);
  });

  it("keeps two seats apart, so the map says who as well as what", () => {
    const m = map();
    const activity = activityOf(
      m,
      [
        member({ doing: "working", slice_key: "M1-S1" }),
        member({ agent_id: 2, role: "frontend", doing: "working", slice_key: "M1-S2" }),
      ],
      [slice("M1-S1", ["crates/**"]), slice("M1-S2", ["ui/**"])],
    );

    expect(activity.hot.get(3)).toBe("backend");
    expect(activity.hot.get(5)).toBe("frontend");
    expect([...activity.working].sort()).toEqual(["backend", "frontend"]);
  });

  it("treats a starting seat as already working", () => {
    // A turn that has been dispatched but has not produced a token yet is still a turn in
    // flight, and a map that waits for the first token looks asleep at the moment a run
    // begins - which is exactly when somebody is watching it.
    const activity = activityOf(
      map(),
      [member({ doing: "starting", slice_key: "M1-S1" })],
      [slice("M1-S1", ["crates/**"])],
    );
    expect(isBusy(activity)).toBe(true);
    expect(activity.hot.size).toBeGreaterThan(0);
  });

  it("stays quiet when a live slice names paths this checkout does not have", () => {
    // The slice is real and the seat is up, but nothing here matches - so nothing lights,
    // rather than something arbitrary doing so.
    const activity = activityOf(
      map(),
      [member({ doing: "working", slice_key: "M1-S1" })],
      [slice("M1-S1", ["ios/**"])],
    );
    expect(activity.hot.size).toBe(0);
    expect(activity.live.size).toBe(0);
    // Still busy: a seat is mid-turn even if the map cannot show where.
    expect(isBusy(activity)).toBe(true);
  });
});
