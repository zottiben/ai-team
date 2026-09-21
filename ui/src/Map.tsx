import { useMemo } from "react";

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

/* Close to 2:1. The panel is a band across the page, not a square canvas - and a taller
 * box makes the fitted layout leave the width empty. */
const WIDTH = 1200;
const HEIGHT = 520;

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
    return layout(nodes, map.edges, { width: WIDTH, height: HEIGHT });
  }, [map]);

  // The viewBox is fitted to what was actually drawn, not to the box the layout was
  // solved in. `fit` fills one axis and centres on the other, so a wide graph in a fixed
  // box leaves a band of empty panel above and below it that reads as a rendering fault.
  const box = useMemo(() => {
    if (points.length === 0) return { x: 0, y: 0, w: WIDTH, h: HEIGHT };
    const pad = 22;
    const spans = points.map((p, at) => ({ p, r: radius(map.nodes[at]?.weight ?? 1) }));
    const minX = Math.min(...spans.map(({ p, r }) => p.x - r)) - pad;
    const maxX = Math.max(...spans.map(({ p, r }) => p.x + r)) + pad;
    const minY = Math.min(...spans.map(({ p, r }) => p.y - r)) - pad;
    const maxY = Math.max(...spans.map(({ p, r }) => p.y + r)) + pad;
    return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
  }, [points, map.nodes]);

  const labels = useMemo(() => {
    return roles
      .map((role) => {
        const mine = points.filter((_, at) => map.nodes[at]?.owner === role);
        const owns = map.zones.find((zone) => zone.role === role)?.owns ?? 0;
        return { role, owns, at: centroid(mine), count: mine.length };
      })
      .filter((label) => label.count > 0);
  }, [roles, points, map]);

  if (map.nodes.length === 0) {
    return <p className="empty">Nothing to map - this checkout has no files ai-team reads.</p>;
  }

  const live = [...activity.live.keys()];

  return (
    <svg
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

      {/* Labels last and on top, haloed so they stay readable over a dense cluster. */}
      <g className="map__labels">
        {labels.map((label) => (
          <text
            key={label.role}
            x={label.at.x}
            y={label.at.y}
            textAnchor="middle"
            data-working={activity.working.has(label.role)}
          >
            {label.role} · {label.owns}
          </text>
        ))}
      </g>
    </svg>
  );
}
