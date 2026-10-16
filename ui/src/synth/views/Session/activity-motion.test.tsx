import { act, render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import type { RunEvent } from "../../../api";
import { currentPending, receiptDelta } from "./activity-motion";
import { SessionPattern } from "./SessionPattern";
import { trackerRows, type TrackerRow } from "./Tracker/rows";

const active = { node: 1, session: "fixture" };
const event = (id: number, call = String(id)): RunEvent => ({ id, actor: "agent", node_run_id: 1, session_id: "fixture", kind: "tool_call", message: null, summary: `Call ${id}`, thinking: [], at: "coarse", tool: { call_id: call, parent_call_id: null, name: "read", target: null, outcome: null, duration_ms: null, output: null } });
const result = (id: number, call: string): RunEvent => ({ ...event(id, call), kind: "tool_result" });

it("marks only a last exact pending observation in the actual active scope", () => {
  const rows = trackerRows([event(1), event(2), result(3, "2")]);
  expect(currentPending(rows, active)).toBeNull(); // An older unreturned call is not guessed current.
  expect(currentPending(trackerRows([event(1), event(2)]), active)).toBe(2);
  expect(currentPending(trackerRows([event(1), { ...event(4, "1"), kind: "note", tool: { ...event(4, "1").tool!, partial: true } }]), active)).toBe(1);
  for (const scope of [null, { ...active, node: 2 }, { ...active, session: "other" }, { ...active, session: "" }]) expect(currentPending(trackerRows([event(1)]), scope)).toBeNull();
  for (const input of [[event(1), event(2, "1")], [{ ...event(1), session_id: null }], [{ ...event(1), tool: { ...event(1).tool!, call_id: null } }]]) expect(currentPending(trackerRows(input), active)).toBeNull();
  expect(currentPending(trackerRows([event(1), { ...event(4), tool: undefined, kind: "thinking", thinking: ["Permitted completed summary"] }]), active)).toBeNull();
});

it("does not replay initial, older, completed, reset, or repeated receipt motion", () => {
  const initial = trackerRows([event(10), result(20, "10")]);
  expect(receiptDelta(initial, null)).toEqual({ highwater: 20, ids: [] });
  expect(receiptDelta(trackerRows([event(5), ...[event(10), result(20, "10")]]), 20)).toEqual({ highwater: 20, ids: [] });
  expect(receiptDelta(trackerRows([event(10), result(30, "10")]), 20)).toEqual({ highwater: 30, ids: [] });
  expect(receiptDelta([], 30)).toEqual({ highwater: 30, ids: [] });
  expect(receiptDelta(trackerRows([event(10), event(30)]), 30).ids).toEqual([]);
  const echo: TrackerRow = { kind: "event", id: 31, event: { ...event(31), kind: "note", actor: "human", message: "Canonical follow-up echo" } };
  expect(receiptDelta([echo], 30, item => item.actor === "human")).toEqual({ highwater: 31, ids: [] });
});

it("bounds new group children without mutating or retaining another history", () => {
  const rows = trackerRows(Array.from({ length: 1000 }, (_, index) => event(index + 1)));
  const before = structuredClone(rows);
  expect(receiptDelta(rows, 10)).toEqual({ highwater: 1000, ids: Array.from({ length: 16 }, (_, index) => 985 + index) });
  expect(rows).toEqual(before);
  expect(receiptDelta([{ kind: "event", id: -2, event: event(-2) }], 0).ids).toEqual([]);
});

it("retains a focused pad when the tail window advances", () => {
  const source = Array.from({ length: 48 }, (_, index) => event(index + 1));
  const view = render(<SessionPattern rows={trackerRows(source)} onSelect={() => {}}/>);
  const first = screen.getByRole("button", { name: /Call 1 · event 1 ·/ });
  act(() => first.focus());
  view.rerender(<SessionPattern rows={trackerRows([...source, event(49)])} onSelect={() => {}}/>);
  expect(first.isConnected).toBe(true); expect(document.activeElement).toBe(first);
  expect(screen.getByText(/held for keyboard focus/)).toBeTruthy();
});

it("shows the observed pending ring without converting summaries or history into liveness", () => {
  const rows: TrackerRow[] = trackerRows([event(1)]);
  const view = render(<SessionPattern rows={rows} active={active} onSelect={() => {}}/>);
  expect(screen.getByRole("button", { name: /Call 1/ }).getAttribute("aria-current")).toBe("step");
  expect(screen.getByRole("button", { name: /Call 1/ }).title).toContain("not proof of liveness");
  view.rerender(<SessionPattern rows={rows} active={null} onSelect={() => {}}/>);
  expect(screen.getByRole("button", { name: /Call 1/ }).getAttribute("aria-current")).toBeNull();
});
