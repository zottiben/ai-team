import type { ChatOverview, ChatSeat, SeatTouch } from "../../../chat-overview-api";

/**
 * Where the agent is in this checkout (SPEC §11.2, §11.4).
 *
 * Everything here is read off evidence the overview already records - the seat's current
 * tool and the paths its tools named. Nothing is inferred from a seat merely existing.
 */
export type Presence = {
  /** A seat holds an unfinished turn *and* a process is behind it. */
  working: boolean;
  role: string | null;
  now: { summary: string; detail: string | null; file: string | null; at: string } | null;
  startedAt: string | null;
  /** Paths touched inside the window, newest first. What the tree lights. */
  recent: SeatTouch[];
  /** Everything it has touched in this checkout, for the monitor's list. */
  touched: SeatTouch[];
  /**
   * The line the agent's last tool reported in the open file.
   *
   * Always null today, and the type says so rather than the component guessing: no
   * provider in Pi's catalogue reports a read offset, line or column with a tool call,
   * so there is nothing to place a caret at. §11.2's fallback - the tag at the top of
   * the viewport - is therefore the normal case, not the error case. The field exists so
   * that a provider which does report one needs no new shape here.
   */
  line: number | null;
};

export const NOBODY: Presence = {
  working: false,
  role: null,
  now: null,
  startedAt: null,
  recent: [],
  touched: [],
  line: null,
};

/** §11.2: "read or edited in the last 5 minutes". */
export const RECENT_MS = 5 * 60 * 1000;

function freshness(at: string, now: number): number {
  const when = Date.parse(at);
  return Number.isNaN(when) ? Number.POSITIVE_INFINITY : now - when;
}

/** The seat that is actually mid-turn, else the one that moved most recently. */
function speaking(seats: ChatSeat[]): ChatSeat | null {
  const live = seats.find((seat) => seat.live && seat.supervised === true);
  if (live !== undefined) return live;
  return (
    [...seats].sort((a, b) => (b.activity?.at ?? "").localeCompare(a.activity?.at ?? ""))[0] ?? null
  );
}

export function presenceOf(overview: ChatOverview | null, now: number): Presence {
  if (overview === null) return NOBODY;
  const seat = speaking(overview.seats);
  if (seat === null) return NOBODY;

  const touched = [...(seat.live_touches ?? []), ...seat.touches];
  // One row per path: a file read five times is one presence light, with the newest
  // timestamp, not five rows arguing about which is current.
  const byPath = new Map<string, SeatTouch>();
  for (const touch of touched) {
    const seen = byPath.get(touch.path);
    if (seen === undefined || seen.at < touch.at) byPath.set(touch.path, touch);
  }
  const all = [...byPath.values()].sort((a, b) => b.at.localeCompare(a.at));

  return {
    working: seat.live && seat.supervised === true,
    role: seat.role,
    now: seat.activity,
    startedAt: seat.started_at,
    recent: all.filter((touch) => freshness(touch.at, now) <= RECENT_MS),
    touched: all,
    line: null,
  };
}

/** Does this path, or a folder on the way to it, hold a recent touch? */
export function presentAt(presence: Presence, path: string, dir: boolean): boolean {
  return presence.recent.some((touch) =>
    dir ? touch.path.startsWith(`${path}/`) : touch.path === path,
  );
}

/** Open files the agent has also touched recently - §11.4's overlap list. */
export function overlapping(presence: Presence, open: string[]): SeatTouch[] {
  return presence.recent.filter((touch) => open.includes(touch.path));
}

/** Whole seconds since the live turn started, or null when nothing is running. */
export function elapsedSeconds(presence: Presence, now: number): number | null {
  if (!presence.working || presence.startedAt === null) return null;
  const started = Date.parse(presence.startedAt);
  if (Number.isNaN(started)) return null;
  return Math.max(0, Math.floor((now - started) / 1000));
}
