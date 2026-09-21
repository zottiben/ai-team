import { anyMatches } from "./glob";
import type { BoardSlice, Member, RepoMap } from "./api";

/**
 * What is moving in the checkout right now.
 *
 * The join that makes the map a live picture rather than a diagram: a seat mid-turn has a
 * `slice_key`, that slice declares the paths it `touches`, and the map knows where those
 * paths are. So the nodes a run is actually editing can be lit, and the route from the
 * root down to them can be drawn as the thing carrying the work.
 *
 * Nothing here is inferred from a seat merely existing. A node is `hot` only because a
 * slice said it would be touched and a seat is mid-turn on that slice - the picture is
 * quiet when the repository is quiet, which is the half of the design that makes the
 * loud half mean anything.
 */
export type Activity = {
  /** Node index to the role touching it: a slice named this path, and its seat is up. */
  hot: Map<number, string>;
  /**
   * Node index to the role that owns it, for a seat working with no slice to narrow it.
   *
   * A turn driven straight at a worktree has no slice behind it, so the honest statement
   * is "this seat is working somewhere in its zone" rather than a guess at which file.
   */
  warm: Map<number, string>;
  /**
   * Edge index to the role whose work is travelling along it, for the routes from the
   * root down to whatever is hot.
   *
   * Carries the role rather than just the fact, so a live route is drawn in the colour of
   * the seat doing the work and two agents running at once read as two things happening
   * rather than one. Near the root a route is shared; the first seat to claim an edge
   * keeps it, which is a tie-break nobody can see and not a statement about ownership.
   */
  live: Map<number, string>;
  /** The roles mid-turn. */
  working: Set<string>;
};

/** The states that mean a seat has a turn in flight. */
const MID_TURN = new Set(["working", "starting"]);

export function activityOf(map: RepoMap, crew: Member[], slices: BoardSlice[]): Activity {
  const hot = new Map<number, string>();
  const warm = new Map<number, string>();
  const live = new Map<number, string>();
  const working = new Set<string>();

  const busy = crew.filter((member) => MID_TURN.has(member.doing));
  if (busy.length === 0) return { hot, warm, live, working };

  const bySlice = new Map(slices.map((slice) => [slice.key, slice]));

  for (const member of busy) {
    working.add(member.role);
    const slice = member.slice_key === null ? undefined : bySlice.get(member.slice_key);
    const touches = slice?.touches ?? [];

    if (touches.length > 0) {
      map.nodes.forEach((node, at) => {
        // The root stands for the whole checkout, so a slice touching anything would
        // light it and the picture would say "everything is being worked on".
        if (node.path !== "" && anyMatches(touches, node.path)) hot.set(at, member.role);
      });
    } else {
      map.nodes.forEach((node, at) => {
        if (node.path !== "" && node.owner === member.role) warm.set(at, member.role);
      });
    }
  }

  // The map is a tree, so the route to a hot node is just the walk up to the root.
  const parentEdge = new Map<number, number>();
  map.edges.forEach((edge, at) => parentEdge.set(edge.to, at));

  for (const [at, role] of hot) {
    let node = at;
    // Bounded against a malformed map rather than against a deep one: the walk should
    // always terminate at the root, and a cycle here would otherwise hang the render.
    for (let step = 0; step < 128; step += 1) {
      const edge = parentEdge.get(node);
      if (edge === undefined) break;
      // Already walked: everything above this edge is live too.
      if (live.has(edge)) break;
      live.set(edge, role);
      const up = map.edges[edge]?.from;
      if (up === undefined) break;
      node = up;
    }
  }

  return { hot, warm, live, working };
}

/** Is anything happening at all? What decides whether the page animates. */
export function isBusy(activity: Activity): boolean {
  return activity.working.size > 0;
}
