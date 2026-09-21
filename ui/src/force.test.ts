import { describe, expect, it } from "vitest";

import { centroid, groupsOf, layout, type ForceEdge, type ForceNode } from "./force";

/** A small repository: two zones, one unowned file, all hanging off a root. */
function repo(): { nodes: ForceNode[]; edges: ForceEdge[] } {
  const nodes: ForceNode[] = [{ id: 0, weight: 9, group: null }];
  const edges: ForceEdge[] = [];
  const add = (weight: number, group: string | null) => {
    nodes.push({ id: nodes.length, weight, group });
    edges.push({ from: 0, to: nodes.length - 1 });
  };
  for (let i = 0; i < 4; i += 1) add(1, "backend");
  for (let i = 0; i < 4; i += 1) add(1, "frontend");
  add(1, null);
  return { nodes, edges };
}

const BOX = { width: 800, height: 500, iterations: 120 };

describe("the map's layout", () => {
  it("draws the same checkout the same way twice", () => {
    // No `Math.random` anywhere: a panel that reshuffles while it is being read is a
    // panel nobody trusts, and two machines must agree about a repository that has not
    // changed (D12).
    const { nodes, edges } = repo();
    expect(layout(nodes, edges, BOX)).toEqual(layout(nodes, edges, BOX));
  });

  it("puts a zone's files together", () => {
    // The whole point of the picture. A layout that only knows about edges draws the
    // directory tree and leaves ownership invisible.
    const { nodes, edges } = repo();
    const points = layout(nodes, edges, BOX);

    const of = (group: string) =>
      points.filter((_, index) => nodes[index]?.group === group);
    const back = centroid(of("backend"));
    const front = centroid(of("frontend"));

    const spread = (group: string, middle: { x: number; y: number }) =>
      Math.max(...of(group).map((p) => Math.hypot(p.x - middle.x, p.y - middle.y)));
    const between = Math.hypot(back.x - front.x, back.y - front.y);

    expect(spread("backend", back)).toBeLessThan(between);
    expect(spread("frontend", front)).toBeLessThan(between);
  });

  it("keeps every node inside the box it was given", () => {
    // Fitted at the end rather than clamped during, so nothing piles up on an edge.
    const { nodes, edges } = repo();
    for (const point of layout(nodes, edges, BOX)) {
      expect(point.x).toBeGreaterThanOrEqual(0);
      expect(point.x).toBeLessThanOrEqual(BOX.width);
      expect(point.y).toBeGreaterThanOrEqual(0);
      expect(point.y).toBeLessThanOrEqual(BOX.height);
    }
  });

  it("separates nodes that start in the same place", () => {
    // Coincident points have no direction to push apart along. Left alone they stay
    // welded together and the cluster renders as one dot.
    const nodes: ForceNode[] = Array.from({ length: 6 }, (_, id) => ({
      id,
      weight: 1,
      group: "backend",
    }));
    const points = layout(nodes, [], BOX);

    points.forEach((a, at) => {
      for (const b of points.slice(at + 1)) {
        expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThan(1);
      }
    });
  });

  it("has nothing to draw for an empty repository", () => {
    expect(layout([], [], BOX)).toEqual([]);
  });

  it("names zones in a stable order rather than the order they arrived", () => {
    expect(
      groupsOf([
        { id: 0, weight: 1, group: "frontend" },
        { id: 1, weight: 1, group: null },
        { id: 2, weight: 1, group: "backend" },
        { id: 3, weight: 1, group: "frontend" },
      ]),
    ).toEqual(["backend", "frontend"]);
  });
});
