import type { RunEvent } from "../../../api";
import type { TrackerRow } from "./Tracker/rows";

export type ActiveScope = { node: number; session: string };
function receipts(row: TrackerRow): RunEvent[] {
  if (row.kind === "event") return [row.event];
  if (row.kind === "thinking") return row.events;
  const tools = row.kind === "reads" ? row.tools : [row];
  return tools.flatMap(tool => [tool.start, tool.update, tool.result].filter((event): event is RunEvent => !!event));
}

/** A pending last observation, not proof of executor liveness. Completed thinking
 * summaries and missing/ambiguous tool identities never become a current call. */
export function currentPending(rows: readonly TrackerRow[], active: ActiveScope | null): number | null {
  if (!active || !Number.isSafeInteger(active.node) || active.node <= 0 || !active.session) return null;
  let latest = 0, current: number | null = null;
  for (const row of rows) {
    for (const event of receipts(row)) {
      if (event.node_run_id !== active.node || event.session_id !== active.session || !Number.isSafeInteger(event.id) || event.id <= latest) continue;
      latest = event.id; current = null;
      const tools = row.kind === "reads" ? row.tools : row.kind === "tool" ? [row] : [];
      const pending = tools.find(tool => tool.lane && tool.correlation === "start_only" && (tool.start?.id === event.id || tool.update?.id === event.id));
      if (pending) current = pending.id;
    }
  }
  return current;
}

/** Monotonic receipt high-water, not a second event history. Returning to a skin,
 * loading older history or completing an existing row must not replay arrivals. */
export function receiptDelta(rows: readonly TrackerRow[], previous: number | null, ownedElsewhere?: (event: RunEvent) => boolean): { highwater: number; ids: number[] } {
  let highwater = previous ?? 0;
  const ids: number[] = [];
  for (const row of rows) {
    for (const event of receipts(row)) if (Number.isSafeInteger(event.id) && event.id > highwater) highwater = event.id;
    if (previous === null || row.kind === "event" && ownedElsewhere?.(row.event)) continue;
    const candidates = row.kind === "thinking" ? row.events.map(event => event.id) : row.kind === "reads" ? row.tools.map(tool => tool.id) : [row.id];
    for (const id of candidates) {
      if (!Number.isSafeInteger(id) || id <= previous || ids.includes(id)) continue;
      ids.push(id);
      if (ids.length > 16) ids.splice(ids.indexOf(Math.min(...ids)), 1);
    }
  }
  return { highwater, ids: ids.sort((a, b) => a - b) };
}
