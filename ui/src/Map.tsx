import { useLayoutEffect, useMemo, useState } from "react";

import type { Activity } from "./activity";
import { centroid, layout, type ForceNode, type Point } from "./force";
import type { RepoMap } from "./api";

/** How many zone colours the token layer defines. A seventh seat reuses the first. */
const ZONE_COLOURS = 6;

/**
 * Which colour a role gets, and the same one every time.
 *
 * Keyed on the role's position in the *sorted* zone list rather than on the roster's
 * order, so adding a seat does not recolour the ones already there - a map that changes
 * palette between two renders is a map you have to re-learn.
 */
export function zoneColour(role: string | null, roles: string[]): string {
  if (role === null) return "var(--zone-none-default)";
  const at = roles.indexOf(role);
  if (at < 0) return "var(--zone-none-default)";
  return `var(--zone-${(at % ZONE_COLOURS) + 1}-default)`;
}

/** A point's radius. Square-rooted, because area is what the eye compares, not width. */
function radius(weight: number): number {
  return Math.min(13, 2.1 + Math.sqrt(Math.max(1, weight)) * 0.85);
}

/* The band the map is solved in when there is nothing to measure it by - a document with
 * no layout, or a panel not on the page yet. Close to 2:1: across the page the panel is a
 * band, not a square canvas, and a taller box makes the fitted layout leave the width
 * empty. */
const WIDTH = 1200;
const HEIGHT = 520;

/** How short a narrow panel's picture may get before it is a strip too thin to read. */
const MIN_HEIGHT = 240;

/** How long a width has to hold before the layout is solved for it again. */
const SETTLE_MS = 180;

/** A width change smaller than this is a scrollbar coming and going, not a new panel. */
const SETTLE_PX = 24;

/** A zone label's size: 11px, as `.map__labels text` sets it. */
const LABEL_SIZE = 11;

/** How far a monospace glyph advances, per unit of size. The label is mono, so its width is
 * this times its length, and the plate under it can be sized without measuring. */
const MONO_ADVANCE = 0.62;

type Drawn = { width: number; maxHeight: number };

type Box = { x: number; y: number; w: number; h: number };

/**
 * Where a zone's label goes: on its cluster, moved up or down half a plate at a time to
 * the nearest place that hides none of the points - in a crowd, the fewest - and never out
 * of the drawing.
 *
 * Its cluster's middle is where that cluster's hub is, so a label left there hides the one
 * point the zone's lines all meet at.
 */
function placeLabel(
  at: Point,
  plate: { w: number; h: number },
  view: Box,
  points: Array<Point & { r: number }>,
): Point {
  const x = Math.min(Math.max(at.x, view.x + plate.w / 2), view.x + view.w - plate.w / 2);
  const inside = (y: number) =>
    Math.min(Math.max(y, view.y + plate.h / 2), view.y + view.h - plate.h / 2);
  const hides = (y: number) =>
    points.filter(
      (p) => Math.abs(p.x - x) < plate.w / 2 + p.r && Math.abs(p.y - y) < plate.h / 2 + p.r,
    ).length;
  let best = { y: inside(at.y), hidden: hides(inside(at.y)) };
  for (const step of [0.5, -0.5, 1, -1, 1.5, -1.5, 2, -2, 3, -3]) {
    if (best.hidden === 0) break;
    const y = inside(at.y + step * plate.h);
    const hidden = hides(y);
    if (hidden < best.hidden) best = { y, hidden };
  }
  return { x, y: best.y };
}

/**
 * The box to solve the layout in, for a picture drawn `width` pixels wide.
 *
 * Solved at the size it is shown rather than for one band and scaled to fit: scaled, a
 * band in one column of the Overview drew its labels three pixels tall and its files as
 * specks. At one unit to the pixel a label is the size the stylesheet says. A band keeps
 * its proportions, a column goes squarer, and neither is taller than the stylesheet lets
 * the panel be.
 */
function boxFor({ width, maxHeight }: Drawn): { width: number; height: number } {
  if (width <= 0) return { width: WIDTH, height: HEIGHT };
  const height = Math.max(MIN_HEIGHT, (width * HEIGHT) / WIDTH);
  return { width, height: Math.min(maxHeight, height) };
}

/**
 * How wide `svg` is drawn, and how tall the stylesheet lets it be.
 *
 * Read before the first paint, so the map never flashes at the wrong scale. After that a
 * resize is followed once it settles: solving a few hundred files takes long enough to
 * make a dragged window stutter, and while it moves the picture it has scales instead.
 */
function useDrawn(svg: SVGSVGElement | null): Drawn {
  const [drawn, setDrawn] = useState<Drawn>({ width: 0, maxHeight: Infinity });
  useLayoutEffect(() => {
    if (svg === null) return;
    const read = (width: number) => {
      // The cap is the stylesheet's - a compact layout lowers it - so it is asked for
      // rather than repeated here. No stylesheet, no cap.
      const cap = Number.parseFloat(getComputedStyle(svg).maxHeight);
      const maxHeight = Number.isFinite(cap) ? cap : Infinity;
      setDrawn((current) =>
        Math.abs(current.width - width) < SETTLE_PX && current.maxHeight === maxHeight
          ? current
          : { width, maxHeight },
      );
    };
    read(svg.getBoundingClientRect().width);
    let settling: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width;
      if (width === undefined) return;
      clearTimeout(settling);
      settling = setTimeout(() => read(width), SETTLE_MS);
    });
    observer.observe(svg);
    return () => {
      observer.disconnect();
      clearTimeout(settling);
    };
  }, [svg]);
  return drawn;
}

/**
 * The checkout as a graph: who owns each path, and what is being worked on right now.
 *
 * Two things at once, and they are deliberately different registers. The *colour* is
 * static - it answers the question the roster could only assert, which is whether
 * `crates/**` actually covers this repository and how much of it nobody claims (D14).
 * The *motion* is entirely live: a node glows because a slice named it and its seat is
 * mid-turn, and the routes from the root down to it carry a flow in that seat's colour.
 *
 * So a quiet repository is a still picture, and a run makes it move. That contrast is the
 * whole point - ambient motion that ran all the time would say nothing.
 *
 * The layout is solved once and memoised on the map, never on the activity: re-running a
 * force simulation on every database tick would move the ground under whoever is reading
 * it, and the repository has not changed shape just because a token arrived.
 */
export function RepoGraph({ map, activity }: { map: RepoMap; activity: Activity }) {
  const [svg, setSvg] = useState<SVGSVGElement | null>(null);
  const drawn = useDrawn(svg);
  const { width, height } = boxFor(drawn);

  const roles = useMemo(
    () => [...new Set(map.zones.map((zone) => zone.role))].sort(),
    [map.zones],
  );

  const points = useMemo<Point[]>(() => {
    const nodes: ForceNode[] = map.nodes.map((node, id) => ({
      id,
      weight: node.weight,
      // The root is deliberately ungrouped, whatever its contents voted for. Anchoring it
      // to the majority zone drags the trunk inside that cluster and leaves the other
      // zones hanging off the far end of one long thread; ungrouped, it settles in the
      // middle and the repository reads as one thing with branches.
      group: node.path === "" ? null : node.owner,
    }));
    return layout(nodes, map.edges, { width, height });
  }, [map, width, height]);

  // Full width, so a unit stays a pixel, but only as tall as what was actually drawn.
  // `fit` fills one axis and centres on the other, so a wide graph leaves a band of empty
  // panel above and below it that reads as a rendering fault. Kept inside the solved box,
  // which is inside the panel's cap: taller would be letterboxed back down.
  const box = useMemo<Box>(() => {
    if (points.length === 0) return { x: 0, y: 0, w: width, h: height };
    const pad = 22;
    const spans = points.map((p, at) => ({ p, r: radius(map.nodes[at]?.weight ?? 1) }));
    const minY = Math.max(0, Math.min(...spans.map(({ p, r }) => p.y - r)) - pad);
    const maxY = Math.min(height, Math.max(...spans.map(({ p, r }) => p.y + r)) + pad);
    return { x: 0, y: minY, w: width, h: maxY - minY };
  }, [points, map.nodes, width, height]);

  const labels = useMemo(() => {
    return roles
      .map((role) => {
        const mine = points.filter((_, at) => map.nodes[at]?.owner === role);
        const owns = map.zones.find((zone) => zone.role === role)?.owns ?? 0;
        return { role, owns, at: centroid(mine), count: mine.length };
      })
      .filter((label) => label.count > 0)
      .map((label) => {
        const text = `${label.role} · ${label.owns}`;
        const plate = { w: text.length * LABEL_SIZE * MONO_ADVANCE + 8, h: LABEL_SIZE + 5 };
        return { role: label.role, text, plate, at: label.at };
      });
  }, [roles, points, map]);

  const placed = useMemo(() => {
    const circles = points.map((p, at) => ({ ...p, r: radius(map.nodes[at]?.weight ?? 1) }));
    return labels.map((label) => ({
      ...label,
      at: placeLabel(label.at, label.plate, box, circles),
    }));
  }, [labels, points, map.nodes, box]);

  if (map.nodes.length === 0) {
    return <p className="empty">Nothing to map - this checkout has no files ai-team reads.</p>;
  }

  const live = [...activity.live.keys()];

  return (
    <svg
      ref={setSvg}
      className="map"
      viewBox={`${box.x} ${box.y} ${box.w} ${box.h}`}
      role="img"
      data-busy={activity.working.size > 0}
      aria-label={
        activity.working.size === 0
          ? `${map.files} files across ${labels.length} zones, nothing running`
          : `${map.files} files across ${labels.length} zones, ${activity.working.size} working`
      }
    >
      {/* Edges first, so a point is never cut through by the line that reaches it. */}
      <g className="map__edges">
        {map.edges.map((edge, at) => {
          const from = points[edge.from];
          const to = points[edge.to];
          if (from === undefined || to === undefined) return null;
          return (
            <line
              key={at}
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              // A live route is drawn twice: once dim underneath with everything else,
              // once again below with the flow on it. Dimming it here keeps the static
              // line from fighting the moving one.
              className={activity.live.has(at) ? "map__edge is-route" : "map__edge"}
            />
          );
        })}
      </g>

      {/* The routes carrying work, on top of the static graph and under the nodes. */}
      <g className="map__flows">
        {live.map((at) => {
          const edge = map.edges[at];
          if (edge === undefined) return null;
          const from = points[edge.from];
          const to = points[edge.to];
          if (from === undefined || to === undefined) return null;
          const role = activity.live.get(at) ?? null;
          return (
            <line
              key={at}
              className="map__flow"
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              stroke={zoneColour(role, roles)}
              // Staggered by depth so the flow reads as travelling outward from the root
              // rather than every segment blinking in unison, which looks like a fault.
              style={{ animationDelay: `${((map.nodes[edge.to]?.depth ?? 0) % 6) * -0.18}s` }}
            />
          );
        })}
      </g>

      <g>
        {map.nodes.map((node, at) => {
          const point = points[at];
          if (point === undefined) return null;
          const hot = activity.hot.get(at);
          const warm = activity.warm.get(at);
          const state = hot !== undefined ? "hot" : warm !== undefined ? "warm" : "still";
          const doing = hot ?? warm ?? null;
          return (
            <circle
              key={node.path === "" ? "__root" : node.path}
              cx={point.x}
              cy={point.y}
              r={radius(node.weight)}
              // A node being worked on takes the colour of whoever is working on it, not
              // of whoever owns it - which are the same thing until they are not, and the
              // moment they differ is the one worth seeing.
              fill={zoneColour(doing ?? node.owner, roles)}
              className={node.dir ? "map__node is-hub" : "map__node"}
              data-state={state}
              // Offset per node so a region of a hundred files breathes like a crowd
              // rather than like one object.
              style={state === "still" ? undefined : { animationDelay: `${(at % 11) * -0.14}s` }}
            >
              {/* The only interaction the picture needs: what am I looking at. */}
              <title>
                {node.path === "" ? map.root : node.path}
                {" · "}
                {doing === null ? (node.owner ?? "unowned") : `${doing} is working here`}
              </title>
            </circle>
          );
        })}
      </g>

      {/* Labels last and on top, each on a plate so it stays readable where it has to
          cross the lines - or, in a crowd, the points - of a dense cluster. */}
      <g className="map__labels">
        {placed.map(({ role, text, plate, at }) => (
          <g key={role}>
            <rect
              className="map__label-plate"
              x={at.x - plate.w / 2}
              y={at.y - plate.h / 2}
              width={plate.w}
              height={plate.h}
              rx={3}
            />
            <text
              x={at.x}
              y={at.y}
              textAnchor="middle"
              dominantBaseline="central"
              data-working={activity.working.has(role)}
            >
              {text}
            </text>
          </g>
        ))}
      </g>
    </svg>
  );
}
