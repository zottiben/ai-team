import { expect, it } from "vitest";
import type { RunEvent } from "../../../api";
import { patternEntries } from "./SessionPattern";
import type { ToolActivityRow } from "./Tracker/rows";

const event = (id: number): RunEvent => ({ id, actor: "agent", node_run_id: 1, session_id: "fixture", kind: "tool_call", message: null, summary: `Call ${id}`, thinking: [], at: "coarse" });
const tool = (id: number): ToolActivityRow => ({ kind: "tool", id, start: event(id), result: null, lane: "read", correlation: "start_only", duration: null, parallel: null });

it("maps exact grouped event identities and never uses observed intervals as pad execution durations", () => {
  const first = { ...tool(1), duration: { source: "observed" as const, milliseconds: 25 } };
  const second = { ...tool(2), duration: { source: "runtime" as const, milliseconds: 0 } };
  const rows = patternEntries([
    { kind: "reads", id: 1, tools: [first, second] },
    { ...tool(3), lane: null },
    { kind: "thinking", id: 4, events: [{ ...event(4), thinking: ["A permitted summary"] }, { ...event(5), thinking: ["Another summary"] }] },
  ]);
  expect(rows.map(row => [row.id, row.row, row.duration])).toEqual([[1, 1, null], [2, 1, 0], [4, 4, null], [5, 4, null]]);
});
