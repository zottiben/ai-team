import type { Activity } from "./activity";
import type { RepoMap } from "./api";
import type { ChatSeat } from "./chat-overview-api";

/**
 * What this chat is moving in its checkout right now.
 *
 * The join that makes the map a live picture rather than a diagram, done from evidence
 * rather than from intent: a seat's own stream says which files its tools opened, and the
 * map knows where those paths are. The project Overview had to infer this from a slice's
 * declared `Touches:`, because a run's seats and a run's files only meet on the board. A
 * chat has the files themselves.
 *
 * Nothing lights because a seat exists. A seat that is not holding a live turn contributes
 * nothing here, so a settled chat is a still picture - which is the half of the design
 * that makes the loud half mean anything.
 */
export function chatActivity(map: RepoMap, seats: ChatSeat[]): Activity {
  const hot = new Map<number, string>();
  const warm = new Map<number, string>();
  const live = new Map<number, string>();
  const working = new Set<string>();

  const at = new Map<string, number>();
  map.nodes.forEach((node, index) => {
    // The root stands for the whole checkout, so touching anything would light it and the
    // picture would say "everything is being worked on".
    if (node.path !== "") at.set(node.path, index);
  });

  for (const seat of seats.filter(isWorking)) {
    working.add(seat.role);
    const current = seat.activity?.file ?? null;
    for (const touch of seat.live_touches ?? seat.touches) {
      const index = at.get(touch.path);
      if (index === undefined) continue;
      // A file it changed is work; a file it only read is context. Two registers, because
      // a seat that read forty files has not edited forty files.
      if (touch.writes > 0) hot.set(index, seat.role);
      else if (!hot.has(index)) warm.set(index, seat.role);
    }
    const now = current === null ? undefined : at.get(current);
    if (now !== undefined) {
      hot.set(now, seat.role);
      warm.delete(now);
    }
  }

  // The map is a tree, so the route to a lit node is the walk up to the root.
  const parent = new Map<number, number>();
  map.edges.forEach((edge, index) => parent.set(edge.to, index));
  for (const [node, role] of hot) {
    let step = node;
    // Bounded against a malformed map rather than a deep one: the walk should always end
    // at the root, and a cycle would otherwise hang the render.
    for (let guard = 0; guard < 128; guard += 1) {
      const edge = parent.get(step);
      if (edge === undefined) break;
      if (live.has(edge)) break;
      live.set(edge, role);
      const up = map.edges[edge]?.from;
      if (up === undefined) break;
      step = up;
    }
  }

  return { hot, warm, live, working };
}

/**
 * A seat holding a turn that something is actually behind.
 *
 * `live` alone is a row that has not been closed; the supervisor check is what separates
 * working from interrupted (rule 6). An interrupted seat keeps its evidence and its place
 * on the card, and must not animate as though it were still thinking.
 */
export function isWorking(seat: ChatSeat): boolean {
  return seat.live && seat.supervised !== false;
}

/** Thousands, because the interesting numbers are six figures. */
export function compact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

export function percent(value: number | null): string {
  return value === null ? "--" : `${Math.round(value * 100)}%`;
}

/**
 * How long ago, from a timestamp the server wrote.
 *
 * SQLite writes `2026-10-06 09:12:03` with no zone, and the server's timestamps are UTC.
 * Parsed as written, a browser reads them as local time and every elapsed figure is out
 * by the offset - which on a machine west of UTC is a negative age.
 */
export function elapsedSince(at: string, now: number): string {
  const parsed = Date.parse(/[Z+]|[+-]\d\d:\d\d$/.test(at) ? at : `${at.replace(" ", "T")}Z`);
  if (!Number.isFinite(parsed)) return "unknown";
  const seconds = Math.max(0, Math.floor((now - parsed) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** What one seat's card says about its newest action, and whether that is happening now. */
export function operational(
  seat: ChatSeat,
  now: number,
): { label: string; summary: string; detail: string | null; timing: string; live: boolean } | null {
  if (seat.activity === null) return null;
  const live = isWorking(seat) && seat.activity.kind === "tool_call";
  const age = elapsedSince(seat.activity.at, now);
  return {
    label: live ? "Current command" : isWorking(seat) ? "Current activity" : "Latest update",
    summary: seat.activity.summary,
    detail: seat.activity.detail,
    timing: live ? `running for ${age}` : age === "0s" ? "just now" : `${age} ago`,
    live,
  };
}
