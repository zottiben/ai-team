import type { RunEvent } from "../../../../api";

export type ActivityLane = "think" | "read" | "edit" | "bash" | "mcp";
export type ActivityDuration = { milliseconds: number; source: "runtime" | "observed" };
export type ToolActivityRow = {
  kind: "tool";
  id: number;
  start: RunEvent | null;
  result: RunEvent | null;
  update?: RunEvent;
  lane: ActivityLane | null;
  correlation: "paired" | "start_only" | "result_only" | "missing_identity" | "ambiguous" | "update_only";
  duration: ActivityDuration | null;
  /** Observed overlapping completed calls, not a claim about executor concurrency. */
  parallel: { first: number; index: number; count: number } | null;
};
export type TrackerRow =
  | ToolActivityRow
  | { kind: "event"; id: number; event: RunEvent }
  | { kind: "thinking"; id: number; events: [RunEvent, ...RunEvent[]] }
  | { kind: "reads"; id: number; tools: [ToolActivityRow, ...ToolActivityRow[]] };

type Indexed = { event: RunEvent; index: number };
type Call = { starts: Indexed[]; results: Indexed[]; updates: Indexed[] };

function scope(event: RunEvent): string | null {
  return event.node_run_id !== null && event.session_id
    ? JSON.stringify([event.node_run_id, event.session_id]) : null;
}
function callKey(event: RunEvent): string | null {
  const owner = scope(event);
  return owner && event.tool?.call_id ? JSON.stringify([owner, event.tool.call_id]) : null;
}
function sameScope(a: RunEvent, b: RunEvent) {
  const owner = scope(a);
  return owner !== null && owner === scope(b);
}
function lane(name: string | null | undefined): ActivityLane | null {
  switch (name) {
    case "read": case "grep": case "find": case "ls": return "read";
    case "write": case "edit": return "edit";
    case "bash": case "powershell": return "bash";
    case "mcp": case "mcpScript": return "mcp";
    default: return name?.startsWith("mcp__") ? "mcp" : null;
  }
}
function duration(start: RunEvent | null, result: RunEvent | null): ActivityDuration | null {
  const runtime = result?.tool?.duration_ms;
  if (typeof runtime === "number" && Number.isFinite(runtime) && runtime >= 0 && runtime <= Number.MAX_SAFE_INTEGER) {
    return { milliseconds: runtime, source: "runtime" };
  }
  const from = start?.observation, to = result?.observation;
  if (!from || !to || !from.clock_id || from.clock_id !== to.clock_id
    || !Number.isSafeInteger(from.elapsed_ms) || !Number.isSafeInteger(to.elapsed_ms)
    || from.elapsed_ms < 0 || to.elapsed_ms < from.elapsed_ms) return null;
  return { milliseconds: to.elapsed_ms - from.elapsed_ms, source: "observed" };
}
function paired(call: Call | undefined): { start: Indexed; result: Indexed } | null {
  if (!call || call.starts.length !== 1 || call.results.length !== 1) return null;
  const start = call.starts[0], result = call.results[0];
  if (!start || !result || start.index >= result.index
    || (start.event.tool?.name && result.event.tool?.name && start.event.tool.name !== result.event.tool.name)) return null;
  return { start, result };
}
function correlation(key: string | null, call: Call | undefined, isStart: boolean): ToolActivityRow["correlation"] {
  if (!key || !call) return "missing_identity";
  if (paired(call)) return "paired";
  if (call.starts.length > 1 || call.results.length > 1 || (call.starts.length && call.results.length)) return "ambiguous";
  return isStart ? "start_only" : "result_only";
}

/** No I/O, clocks, mutated events or guesses by tool name. The caller keeps the one
 * event cursor; re-projecting a later page preserves each start row's identity. */
export function trackerRows(input: readonly RunEvent[]): TrackerRow[] {
  const seen = new Set<number>();
  const events = input.filter(event => {
    if (seen.has(event.id)) return false;
    seen.add(event.id);
    return true;
  });
  const calls = new Map<string, Call>();
  events.forEach((event, index) => {
    if (event.kind !== "tool_call" && event.kind !== "tool_result" && !event.tool?.partial) return;
    const key = callKey(event);
    if (!key) return;
    let call = calls.get(key);
    if (!call) { call = { starts: [], results: [], updates: [] }; calls.set(key, call); }
    (event.tool?.partial ? call.updates : event.kind === "tool_call" ? call.starts : call.results).push({ event, index });
  });

  const attached = new Set<number>(), latest = new Map<number, RunEvent>();
  for (const call of calls.values()) {
    const start = call.starts[0];
    if (call.starts.length !== 1 || !start || call.results.length > 1 || (call.results.length && !paired(call))) continue;
    for (const update of call.updates) {
      if (update.index <= start.index || update.index >= (call.results[0]?.index ?? Infinity)
        || update.event.tool?.name !== start.event.tool?.name) continue;
      attached.add(update.event.id);
      latest.set(start.event.id, update.event);
    }
  }
  const rows: TrackerRow[] = [];
  const overlaps = new Map<string, { row: ToolActivityRow; start: number; end: number }[]>();
  for (const event of events) {
    if (event.tool?.partial) {
      if (!attached.has(event.id)) rows.push({ kind: "tool", id: event.id, start: null, result: null, update: event,
        lane: lane(event.tool.name), correlation: "update_only", duration: null, parallel: null });
      continue;
    }
    if (event.kind !== "tool_call" && event.kind !== "tool_result") {
      if (event.message === null && event.thinking.length) rows.push({ kind: "thinking", id: event.id, events: [event] });
      else rows.push({ kind: "event", id: event.id, event });
      continue;
    }
    const key = callKey(event), call = key ? calls.get(key) : undefined;
    const pair = paired(call);
    const isStart = event.kind === "tool_call";
    const state = correlation(key, call, isStart);
    if (!isStart && pair) continue;
    const start = isStart ? event : null;
    const result = pair ? pair.result.event : isStart ? null : event;
    const row: ToolActivityRow = { kind: "tool", id: event.id, start, result, update: latest.get(event.id),
      lane: lane((start ?? result)?.tool?.name), correlation: state,
      duration: duration(start, result), parallel: null };
    rows.push(row);
    if (pair) {
      const owner = JSON.stringify([scope(event), event.tool?.parent_call_id ?? null]);
      const list = overlaps.get(owner) ?? [];
      list.push({ row, start: pair.start.index, end: pair.result.index });
      overlaps.set(owner, list);
    }
  }
  for (const list of overlaps.values()) {
    let group: ToolActivityRow[] = [], until = -1;
    const finish = () => {
      const first = group[0];
      if (first && group.length > 1) group.forEach((row, index) => {
        row.parallel = { first: first.id, index, count: group.length };
      });
    };
    for (const item of list) {
      if (item.start >= until) { finish(); group = []; }
      group.push(item.row);
      until = Math.max(item.end, until);
    }
    finish();
  }

  const grouped: TrackerRow[] = [];
  for (const row of rows) {
    const previous = grouped.at(-1);
    if (row.kind === "thinking" && previous?.kind === "thinking" && sameScope(previous.events[0], row.events[0])) {
      previous.events.push(...row.events);
    } else if (row.kind === "tool" && row.lane === "read" && row.start) {
      const first = previous?.kind === "reads" ? previous.tools[0] : previous?.kind === "tool" ? previous : null;
      if (first?.lane === "read" && first.start && sameScope(first.start, row.start)) {
        if (previous?.kind === "reads") previous.tools.push(row);
        else grouped[grouped.length - 1] = { kind: "reads", id: first.id, tools: [first, row] };
      } else grouped.push(row);
    } else grouped.push(row);
  }
  return grouped;
}
