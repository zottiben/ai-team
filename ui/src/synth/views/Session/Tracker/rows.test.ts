import { describe, expect, it } from "vitest";
import type { RunEvent } from "../../../../api";
import { trackerRows, type ToolActivityRow } from "./rows";

function event(id: number, kind: string, call: string | null = null, extra: Partial<RunEvent> = {}): RunEvent {
  return {
    id, kind, node_run_id: 10, session_id: "session-a", actor: "agent", summary: "evidence",
    message: null, thinking: [], at: "2026-01-01T00:00:00Z",
    tool: kind.startsWith("tool_") ? {
      name: "bash", call_id: call, parent_call_id: null, target: null,
      outcome: kind === "tool_result" ? "returned" : null, duration_ms: null, output: null,
    } : null,
    ...extra,
  };
}
function tools(events: RunEvent[]): ToolActivityRow[] {
  return trackerRows(events).filter((row): row is ToolActivityRow => row.kind === "tool");
}

describe("tracker evidence projection", () => {
  it("pairs interleaved same-name calls by exact identity and keeps the start row stable", () => {
    const a = event(1, "tool_call", "a"), b = event(2, "tool_call", "b");
    const partial = tools([a, b]);
    expect(partial.map(row => row.id)).toEqual([1, 2]);
    const rows = tools([a, b, event(3, "tool_result", "b"), event(4, "tool_result", "a")]);
    expect(rows).toHaveLength(2);
    expect(rows.map(row => row.id)).toEqual([1, 2]);
    expect(rows.map(row => row.result?.id)).toEqual([4, 3]);
    expect(rows.map(row => row.correlation)).toEqual(["paired", "paired"]);
    expect(rows.map(row => row.parallel)).toEqual([{ first: 1, index: 0, count: 2 }, { first: 1, index: 1, count: 2 }]);
  });

  it("does not cross node/session boundaries or guess missing identities from names", () => {
    const rows = tools([event(1, "tool_call", "same"),
      event(2, "tool_result", "same", { node_run_id: 11 }),
      event(3, "tool_result", "same", { session_id: "session-b" }),
      event(4, "tool_call"), event(5, "tool_result"),
      event(6, "tool_call", "old", { session_id: null }), event(7, "tool_result", "old", { session_id: null })]);
    expect(rows).toHaveLength(7);
    expect(rows.every(row => row.correlation !== "paired")).toBe(true);
    expect(rows.every(row => row.duration === null && row.parallel === null)).toBe(true);
  });

  it("keeps ambiguous duplicate calls/results and out-of-order evidence separate", () => {
    for (const events of [
      [event(1, "tool_call", "a"), event(2, "tool_call", "a"), event(3, "tool_result", "a")],
      [event(1, "tool_call", "a"), event(2, "tool_result", "a"), event(3, "tool_result", "a")],
      [event(1, "tool_result", "a"), event(2, "tool_call", "a")],
    ]) {
      const rows = tools(events);
      expect(rows).toHaveLength(events.length);
      expect(rows.every(row => row.correlation === "ambiguous")).toBe(true);
    }
  });

  it("deduplicates repeated pages by event ID, not by prose or tool name", () => {
    const start = event(1, "tool_call", "a"), end = event(2, "tool_result", "a");
    expect(tools([start, start, end, end])).toHaveLength(1);
    const message = event(3, "note", null, { message: "same" });
    expect(trackerRows([message, { ...message, id: 4 }])).toHaveLength(2);
    expect(tools([end])[0]?.correlation).toBe("result_only");
    expect(tools([start, end])[0]?.id).toBe(start.id);
  });

  it("never derives duration from coarse dates; runtime and host observations remain distinct", () => {
    const start = event(1, "tool_call", "a", { observation: { clock_id: "clock", elapsed_ms: 100, at: "future" } });
    const end = event(2, "tool_result", "a", { observation: { clock_id: "clock", elapsed_ms: 175, at: "past" } });
    expect(tools([start, end])[0]?.duration).toEqual({ milliseconds: 75, source: "observed" });
    const runtime = { ...end, tool: { ...end.tool!, duration_ms: 50.25 } };
    expect(tools([start, runtime])[0]?.duration).toEqual({ milliseconds: 50.25, source: "runtime" });
    expect(tools([{ ...start, observation: null }, { ...end, observation: null }])[0]?.duration).toBeNull();
    for (const observation of [{ ...end.observation!, clock_id: "restarted" }, { ...end.observation!, elapsed_ms: 99 }]) {
      expect(tools([start, { ...end, observation }])[0]?.duration).toBeNull();
    }
    expect(tools([{ ...end, tool: { ...end.tool!, duration_ms: 0 } }])[0]?.duration).toEqual({ milliseconds: 0, source: "runtime" });
  });

  it("groups only consecutive reads/thinking in the same exact scope, preserving prose and source events", () => {
    const read = (id: number, call: string) => {
      const item = event(id, "tool_call", call);
      return { ...item, tool: { ...item.tool!, name: "read" } };
    };
    const think = (id: number) => event(id, "cost", null, { thinking: [`summary ${id}`] });
    const human = event(3, "note", null, { actor: "human", message: "keep this", tool: null });
    const rows = trackerRows([read(1, "a"), read(2, "b"), human, read(4, "c"), think(5), think(6),
      { ...think(7), node_run_id: 20 }, event(8, "failed", null)]);
    expect(rows.map(row => row.kind)).toEqual(["reads", "event", "tool", "thinking", "thinking", "event"]);
    expect(rows[0]).toMatchObject({ id: 1, tools: [{ id: 1 }, { id: 2 }] });
    expect(rows[1]).toMatchObject({ event: human });
    expect(rows[3]).toMatchObject({ id: 5, events: [{ id: 5 }, { id: 6 }] });
  });

  it("keeps mismatched tool names, parent scopes and unknown lanes honest", () => {
    const start = event(1, "tool_call", "a"), end = event(2, "tool_result", "a");
    expect(tools([start, { ...end, tool: { ...end.tool!, name: "read" } }])).toHaveLength(2);
    expect(tools([{ ...start, tool: { ...start.tool!, name: "custom-plugin" } }])[0]?.lane).toBeNull();
    const nested = event(3, "tool_call", "nested");
    nested.tool!.parent_call_id = "a";
    expect(tools([start, nested, event(4, "tool_result", "nested"), end]).every(row => row.parallel === null)).toBe(true);
  });

  it("retains all 5k calls without inventing results or retaining a second payload copy", () => {
    const events = Array.from({ length: 5_000 }, (_, i) => event(i + 1, "tool_call", `c${i}`));
    const rows = tools(events);
    expect(rows).toHaveLength(5_000);
    expect(rows[4_999]?.start).toBe(events[4_999]);
    expect(rows.every(row => row.result === null && row.duration === null)).toBe(true);
  });
});
