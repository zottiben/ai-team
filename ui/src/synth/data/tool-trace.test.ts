import { expect, it } from "vitest";
import type { RunEvent } from "../../api";
import { toolDurations } from "./tool-trace";

function event(id: number, kind: string, duration: number | null, extra: Partial<RunEvent> = {}): RunEvent {
  return { id, node_run_id: 1, kind, actor: "assistant", summary: "Tool receipt", message: null, thinking: [], at: "", session_id: "session",
    observation: { clock_id: "reader", elapsed_ms: id * 1000, at: "" },
    tool: { call_id: "call", parent_call_id: null, name: "bash", target: null, outcome: "returned", duration_ms: duration, output: null }, ...extra };
}

it("plots explicit runtime including zero, never observed receive duration", () => {
  expect(toolDurations([event(1, "tool_call", null), event(2, "tool_result", 0)], 1)).toEqual([0]);
  expect(toolDurations([event(1, "tool_call", null), event(2, "tool_result", null)], 1)).toEqual([null]);
  expect(toolDurations([event(1, "tool_result", 1.5)], 1)).toEqual([1.5]);
});
it("rejects ambiguous completions and keeps the first immutable id before narrowing owners", () => {
  expect(toolDurations([event(1, "tool_call", null), event(2, "tool_result", 20), event(3, "tool_result", 30)], 1)).toEqual([null, null]);
  const first = event(1, "tool_result", 20);
  expect(toolDurations([first, { ...first, node_run_id: 2 }], 2)).toEqual([]);
  expect(toolDurations([first, event(1, "tool_result", 50)], 1)).toEqual([20]);
});
it("leaves missing identities, invalid runtime and partial output unknown", () => {
  for (const value of [-1, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) expect(toolDurations([event(1, "tool_result", value)], 1)).toEqual([null]);
  expect(toolDurations([event(1, "tool_result", 20, { session_id: null })], 1)).toEqual([null]);
  const partial = event(1, "tool_result", 20); if (!partial.tool) throw Error("Missing tool"); partial.tool.partial = true;
  expect(toolDurations([partial], 1)).toEqual([]);
});
it("orders overlapping tools by result receipt, not their starting time, and breaks reader changes", () => {
  const other = event(2, "tool_call", null); if (!other.tool) throw Error("Missing tool"); other.tool.call_id = "other";
  const result = { ...other, id: 3, kind: "tool_result", tool: { ...other.tool, duration_ms: 5 } };
  expect(toolDurations([event(1, "tool_call", null), other, result, event(4, "tool_result", 100)], 1)).toEqual([5, 100]);
  expect(toolDurations([result, event(4, "tool_result", 100, { session_id: "next" })], 1)).toEqual([5, null, 100]);
});
it("bounds the display and never manufactures a pending duration", () => {
  const events = Array.from({ length: 5000 }, (_, i) => event(i, "tool_result", i, { session_id: `session-${i}` }));
  const values = toolDurations(events, 1);
  expect(values).toHaveLength(120); expect(values.at(-1)).toBe(4999);
  expect(toolDurations([event(1, "tool_call", null)], 1)).toEqual([]);
});
