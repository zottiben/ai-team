/**
 * A force-directed layout, written out rather than depended on.
 *
 * It is about a hundred lines for the one picture that needs it, against a graph library
 * that would arrive with a renderer, a transition system and a d3 dependency tree to
 * audit. The same reasoning the updater's SHA-256 was written under: small, fixed, and
 * testable against its own properties.
 *
 * Two things it must do that a stock simulation does not:
 *
 * - **Be deterministic.** No `Math.random`. The same checkout has to draw the same map
 *   every render, or a panel reshuffles itself while somebody is reading it - and on two
 *   machines it would disagree about a repository that had not changed (D12).
 * - **Cluster by zone.** Bodies are pulled toward their owner's anchor as well as toward
 *   their parent, because the thing being shown is *who owns what* - a layout that only
 *   knows about edges draws the directory tree and leaves ownership invisible.
 *
 * Everything is held in one array of bodies rather than several parallel ones. Under
 * `noUncheckedIndexedAccess` every index into a parallel array is a separate thing the
 * checker makes you prove, and the arithmetic disappears behind the proofs.
 */

export type ForceNode = {
  /** Index into the caller's own array; carried through so the result can be zipped back. */
  id: number;
  /** Files at or under this node. Heavier bodies move less, so the trunk stays put. */
  weight: number;
  /** The owner to cluster by, or null for whatever nobody claims. */
  group: string | null;
};

export type ForceEdge = { from: number; to: number };

export type Point = { x: number; y: number };

export type LayoutOptions = {
  width: number;
  height: number;
  /** More is steadier, and costs time. 220 settles a few hundred bodies. */
  iterations?: number;
};

type Body = {
  x: number;
  y: number;
  pushX: number;
  pushY: number;
  mass: number;
  home: Point;
  near: number[];
};

/**
 * Deterministic pseudo-randomness.
 *
 * Seeded from the body's own identity rather than a counter, so adding a file changes
 * where that file starts and not where every later one does.
 */
function scatter(seed: number): number {
  let t = (seed + 0x6d2b79f5) | 0;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

/**
 * Where each zone is anchored: evenly around an ellipse, in a stable order.
 *
 * An ellipse rather than a circle, because the panel is twice as wide as it is tall. On a
 * circle three zones land almost vertically above one another, the layout comes out tall
 * and narrow, and fitting it to the box - which preserves the aspect, or the picture
 * would be stretched - leaves most of the width empty.
 */
function anchors(groups: string[], width: number, height: number): Map<string, Point> {
  const out = new Map<string, Point>();
  // Scaled to how many zones there are. A fixed radius puts two clusters at opposite
  // ends of the panel with one long thread between them and nothing in the middle -
  // which reads as two graphs rather than one repository.
  const spread = 0.1 + Math.min(groups.length, 6) * 0.025;
  const rx = width * spread;
  const ry = height * spread * 0.85;
  groups.forEach((group, index) => {
    // Half a step of rotation, so an even number of zones straddles the middle rather
    // than stacking one directly above another.
    const turn = (index + 0.5) / Math.max(1, groups.length);
    const angle = turn * Math.PI * 2 - Math.PI / 2;
    out.set(group, {
      x: width / 2 + Math.cos(angle) * rx,
      y: height / 2 + Math.sin(angle) * ry,
    });
  });
  return out;
}

/** The zones present, in a stable order - sorted, never in arrival order. */
export function groupsOf(nodes: ForceNode[]): string[] {
  return [...new Set(nodes.map((node) => node.group).filter((g): g is string => g !== null))].sort();
}

export function layout(
  nodes: ForceNode[],
  edges: ForceEdge[],
  { width, height, iterations = 220 }: LayoutOptions,
): Point[] {
  const count = nodes.length;
  if (count === 0) return [];

  const anchor = anchors(groupsOf(nodes), width, height);
  const centre = { x: width / 2, y: height / 2 };

  const bodies: Body[] = nodes.map((node, index) => {
    const home = node.group === null ? centre : (anchor.get(node.group) ?? centre);
    return {
      // Started near their own zone rather than at random: a simulation that begins
      // sorted needs far fewer iterations to look sorted, and the jitter is what keeps
      // co-located bodies from sitting exactly on top of each other for ever.
      x: home.x + (scatter(index * 2 + 1) - 0.5) * width * 0.4,
      y: home.y + (scatter(index * 2 + 2) - 0.5) * height * 0.4,
      pushX: 0,
      pushY: 0,
      // Heavier bodies resist being pushed, so directories settle as hubs and their files
      // orbit rather than the other way round.
      mass: 1 + Math.sqrt(Math.max(1, node.weight)),
      home,
      near: [],
    };
  });

  for (const edge of edges) {
    const from = bodies[edge.from];
    const to = bodies[edge.to];
    if (from === undefined || to === undefined) continue;
    from.near.push(edge.to);
    to.near.push(edge.from);
  }

  // Area per body, capped. Uncapped, a small repository gets an enormous share of the
  // canvas each and the graph comes out as a few threads strung across an empty panel -
  // the cap is what keeps twenty files looking like a repository rather than a constellation.
  const repulsion = Math.min((width * height) / count / 6, 1100);
  // Stiff enough that a directory sits near its parent. Slack springs let the zone
  // anchors win outright, and the result is a few clusters at the edges of the panel
  // joined by long bare threads - a diagram of the anchors rather than of the repository.
  const spring = 0.07;
  const rest = Math.min(width, height) / 26;

  for (let step = 0; step < iterations; step += 1) {
    // Cooling: big moves early, small ones late. Without it the last iterations undo the
    // structure the first ones found.
    const heat = 1 - step / iterations;

    for (const body of bodies) {
      body.pushX = 0;
      body.pushY = 0;
    }

    bodies.forEach((a, ai) => {
      for (let bi = ai + 1; bi < count; bi += 1) {
        const b = bodies[bi];
        if (b === undefined) continue;

        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let distance = Math.hypot(dx, dy);
        if (distance < 0.01) {
          // Exactly coincident bodies have no direction to separate along, so they are
          // given one - deterministically, or the map stops being reproducible.
          dx = scatter(ai * 31 + bi) - 0.5;
          dy = scatter(bi * 31 + ai) - 0.5;
          distance = 0.01;
        }
        const force = repulsion / (distance * distance);
        const fx = (dx / distance) * force;
        const fy = (dy / distance) * force;
        a.pushX += fx;
        a.pushY += fy;
        b.pushX -= fx;
        b.pushY -= fy;
      }
    });

    for (const body of bodies) {
      for (const at of body.near) {
        const other = bodies[at];
        if (other === undefined) continue;
        const dx = other.x - body.x;
        const dy = other.y - body.y;
        const distance = Math.hypot(dx, dy) || 0.01;
        const force = (distance - rest) * spring;
        body.pushX += (dx / distance) * force;
        body.pushY += (dy / distance) * force;
      }

      // Toward its zone, and gently toward the middle so an unowned body does not drift
      // off the canvas with nothing to hold it. The zone pull has to beat the repulsion
      // between two clusters, or they end up at opposite edges joined by one long thread.
      body.pushX += (body.home.x - body.x) * 0.02 + (centre.x - body.x) * 0.006;
      body.pushY += (body.home.y - body.y) * 0.02 + (centre.y - body.y) * 0.006;
    }

    const limit = 18 * heat;
    for (const body of bodies) {
      body.x += Math.max(-limit, Math.min(limit, body.pushX / body.mass));
      body.y += Math.max(-limit, Math.min(limit, body.pushY / body.mass));
    }
  }

  // Fitted to the box at the end rather than clamped during, which would pile bodies up
  // against the edges and call it a layout.
  return fit(
    bodies.map((body) => ({ x: body.x, y: body.y })),
    width,
    height,
  );
}

/** Scale and centre the result so it fills the box it was drawn for. */
function fit(points: Point[], width: number, height: number): Point[] {
  const pad = 28;
  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);

  const spanX = maxX - minX || 1;
  const spanY = maxY - minY || 1;
  const scale = Math.min((width - pad * 2) / spanX, (height - pad * 2) / spanY);

  const offsetX = (width - spanX * scale) / 2;
  const offsetY = (height - spanY * scale) / 2;

  return points.map((p) => ({
    x: (p.x - minX) * scale + offsetX,
    y: (p.y - minY) * scale + offsetY,
  }));
}

/** The middle of a set of points, for putting a zone's label on its own cluster. */
export function centroid(points: Point[]): Point {
  if (points.length === 0) return { x: 0, y: 0 };
  const sum = points.reduce((acc, p) => ({ x: acc.x + p.x, y: acc.y + p.y }), { x: 0, y: 0 });
  return { x: sum.x / points.length, y: sum.y / points.length };
}
