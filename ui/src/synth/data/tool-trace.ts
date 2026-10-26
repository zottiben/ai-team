import type { RunEvent } from "../../api";
import { trackerRows, type ToolActivityRow } from "../views/Session/Tracker/rows";
import { RECEIPTS, SAMPLES } from "./telemetry";

/** Completed execute() durations, not receive intervals or a clock for pending tools.
 * Reuse Tracker's exact call correlation, but display in completion receipt order. */
export function toolDurations(events: readonly RunEvent[], node: number): (number | null)[] {
  const seen = new Set<number>();
  const owned = events.filter(event => {
    if (seen.has(event.id)) return false;
    seen.add(event.id);
    return event.node_run_id === node;
  }).slice(-RECEIPTS);
  const results = new Map<number, ToolActivityRow>();
  for (const row of trackerRows(owned)) {
    for (const tool of row.kind === "reads" ? row.tools : row.kind === "tool" ? [row] : []) {
      if (tool.result) results.set(tool.result.id, tool);
    }
  }
  const values: (number | null)[] = [];
  let previous: string | null = null;
  for (const event of owned) {
    if (event.telemetry?.kind === "compaction") {
      if (values.length) values.push(null);
      previous = null;
      continue;
    }
    if (event.kind !== "tool_result" || event.tool?.partial) continue;
    const key = event.session_id && event.observation?.clock_id ? JSON.stringify([node, event.session_id, event.observation.clock_id]) : null;
    if (values.length && (key === null || key !== previous)) values.push(null);
    const tool = results.get(event.id);
    values.push(tool && (tool.correlation === "paired" || tool.correlation === "result_only") && tool.duration?.source === "runtime" ? tool.duration.milliseconds : null);
    previous = key;
  }
  return values.slice(-SAMPLES);
}
