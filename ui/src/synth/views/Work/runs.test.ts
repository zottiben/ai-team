import { expect, it } from "vitest";
import type { RunEvent } from "../../../api";
import type { ChatDetail, ChatTurn } from "../../../chat-api";
import {
  arrangementScale, clipActivity, counters, durationLabel, durationMs, firstLine,
  instant, matches, ruler, runRecords, runSignal,
} from "./runs";

const node = (over: Partial<ChatTurn["node"]> = {}): ChatTurn["node"] => ({
  id: 1, role: "assistant", provider: "openai", model: "gpt-5", status: "done", attempt: 1,
  slice_key: null, worktree_path: null, branch: null, blocked_reason: null,
  started_at: null, ended_at: null, usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 }, ...over,
});
const run = (over: Partial<ChatTurn["run"]> = {}): ChatTurn["run"] => ({
  id: 1, project_id: 1, prompt: "Do the work", status: "done", trigger: "manual",
  workspace_path: "/repo", created_at: "2026-10-15T09:00:00Z", started_at: "2026-10-15T09:00:00Z",
  ended_at: "2026-10-15T09:30:00Z", ...over,
});
const turn = (over: Partial<ChatTurn> = {}): ChatTurn => ({ run: run(), node: node(), team: null, members: [], ...over });
const detail = (turns: ChatTurn[], over: Partial<ChatDetail> = {}): ChatDetail => ({
  id: 4, project_id: 1, title: "Work", workspace_path: "/repo", provider: "openai", model: "gpt-5",
  reasoning: "high", mode: "single", active_node_id: null, live_text: "", stop_requested: false,
  archived: false, rev: 1, created_at: "", updated_at: "", turns, followups: [], team_builds: [],
  state: "idle", can_resume: false, orphan_running: false, ...over,
});

it("reads a stamp as UTC whether or not the row stored a zone", () => {
  expect(instant("2026-10-15T09:00:00Z")).toBe(Date.UTC(2026, 9, 15, 9, 0, 0));
  expect(instant("2026-10-15 09:00:00")).toBe(Date.UTC(2026, 9, 15, 9, 0, 0));
  expect(instant("2026-10-15T11:00:00+02:00")).toBe(Date.UTC(2026, 9, 15, 9, 0, 0));
  expect(instant(null)).toBeNull();
  expect(instant("")).toBeNull();
  expect(instant("not a time")).toBeNull();
});

it("keeps every persisted state distinct instead of folding it into done or cancelled", () => {
  const states = ["done", "cancelled", "failed", "blocked", "queued", "parked"];
  const records = runRecords(detail(states.map((status, index) => turn({
    run: run({ id: index + 1, status }), node: node({ id: index + 1, status }),
  }))));
  expect(records.map(record => record.status)).toEqual(states);
  expect(records.map(record => record.label)).toEqual(["Done", "Cancelled", "Failed", "Blocked", "Queued", "Parked"]);
  expect(records.map(record => record.kind)).toEqual(["done", "cancelled", "other", "other", "other", "other"]);
  expect(records.every(record => !record.live)).toBe(true);
  expect(counters(records).others.map(other => `${other.label} ${other.count}`)).toEqual(["Failed 1", "Blocked 1", "Queued 1", "Parked 1"]);
});

it("treats only the supervised active turn as live, and never an interrupted one", () => {
  const rows = [turn({ run: run({ id: 1 }), node: node({ id: 1 }) }), turn({ run: run({ id: 2, status: "running", ended_at: null }), node: node({ id: 2, status: "running" }) })];
  const working = runRecords(detail(rows, { active_node_id: 2, state: "running" }));
  expect(working.map(record => record.live)).toEqual([false, true]);
  expect(working[1]?.label).toBe("Working");
  const stopping = runRecords(detail(rows, { active_node_id: 2, state: "stopping" }));
  expect(stopping[1]).toMatchObject({ live: true, label: "Stopping…", status: "stopping" });
  const interrupted = runRecords(detail(rows, { active_node_id: 2, state: "interrupted" }));
  expect(interrupted[1]).toMatchObject({ live: false, label: "Interrupted", status: "interrupted", kind: "other" });
});

it("gives a team turn its phase, and says when the team needs recovery", () => {
  const team = { run_id: 7, chat_id: 4, control_node_id: 7, phase: "awaiting_approval" as const, base_sha: null, approved_revision: null, reason: null, rev: 2, quiescent: true };
  const rows = [turn({ run: run({ id: 7, status: "running", ended_at: null }), node: node({ id: 7, status: "running" }), team })];
  const waiting = runRecords(detail(rows, { active_node_id: 7, state: "awaiting_approval" }));
  expect(waiting[0]).toMatchObject({ label: "Awaiting your approval", status: "awaiting_approval", live: false, kind: "other" });
  const recovering = runRecords(detail(rows, { active_node_id: 7, state: "team_interrupted" }));
  expect(recovering[0]).toMatchObject({ label: "Team needs recovery", live: false });
  const building = runRecords(detail([turn({ run: run({ id: 7, status: "running", ended_at: null }), node: node({ id: 7, status: "running" }), team: { ...team, phase: "building" } })], { active_node_id: 7, state: "running" }));
  expect(building[0]).toMatchObject({ label: "Building approved slices", live: true });
});

it("measures wall duration, keeps a measured zero and refuses to invent a missing end", () => {
  const now = Date.UTC(2026, 9, 15, 10, 0, 0);
  const [done, stopped, instantRun, live] = runRecords(detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "running", ended_at: null }), node: node({ id: 2, status: "running" }) }),
    turn({ run: run({ id: 3, started_at: "2026-10-15T09:00:00Z", ended_at: "2026-10-15T09:00:00Z" }), node: node({ id: 3 }) }),
    turn({ run: run({ id: 4, status: "running", started_at: "2026-10-15T09:45:00Z", ended_at: null }), node: node({ id: 4, status: "running" }) }),
  ], { active_node_id: 4, state: "running" }));
  expect(durationMs(done!, now)).toBe(30 * 60_000);
  expect(durationMs(stopped!, now)).toBeNull();
  expect(durationMs(instantRun!, now)).toBe(0);
  expect(durationMs(live!, now)).toBe(15 * 60_000);
  expect(durationLabel(durationMs(stopped!, now))).toBe("duration unknown");
  expect(durationLabel(0)).toBe("0s");
  expect(durationLabel(45_000)).toBe("45s");
  expect(durationLabel(90_000)).toBe("1m 30s");
  expect(durationLabel(5_400_000)).toBe("1h 30m");
});

it("does not mix node and run clocks or turn reversed timestamps into zero", () => {
  const [missing, reversed] = runRecords(detail([
    turn({ run: run({ ended_at: null }), node: node({ ended_at: "2026-10-15T09:30:00Z" }) }),
    turn({ run: run({ ended_at: "2026-10-15T08:00:00Z" }) }),
  ]));
  expect(durationMs(missing!, Date.UTC(2026, 9, 15, 10))).toBeNull();
  expect(durationMs(reversed!, Date.UTC(2026, 9, 15, 10))).toBeNull();
});

it("falls back to equal widths and no ruler as soon as one run cannot be measured", () => {
  expect(arrangementScale([600_000, 1_200_000])).toMatchObject({ proportional: true, totalMs: 1_800_000 });
  expect(arrangementScale([600_000, null])).toMatchObject({ proportional: false, totalMs: 0, ticks: [] });
  expect(arrangementScale([0, 0])).toMatchObject({ proportional: false, ticks: [] });
  expect(arrangementScale([])).toMatchObject({ proportional: false, ticks: [] });
});

it("rules a long lane in 30-minute marks and a short one in marks it can show", () => {
  expect(ruler(90 * 60_000).map(tick => tick.label)).toEqual(["0m", "30m", "60m", "90m"]);
  expect(ruler(90 * 60_000).map(tick => Number(tick.fraction.toFixed(3)))).toEqual([0, 0.333, 0.667, 1]);
  expect(ruler(6 * 60_000).map(tick => tick.label)).toEqual(["0m", "1m", "2m", "3m", "4m", "5m", "6m"]);
  expect(ruler(8 * 3_600_000).map(tick => tick.label)).toEqual(["0h", "2h", "4h", "6h", "8h"]);
  expect(ruler(0)).toEqual([]);
});

it("counts and filters without hiding a state", () => {
  const records = runRecords(detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "cancelled" }), node: node({ id: 2, status: "cancelled" }) }),
    turn({ run: run({ id: 3, status: "failed" }), node: node({ id: 3, status: "failed" }) }),
    turn({ run: run({ id: 4, status: "running", ended_at: null }), node: node({ id: 4, status: "running" }) }),
  ], { active_node_id: 4, state: "running" }));
  expect(counters(records)).toMatchObject({ runs: 4, done: 1, cancelled: 1, live: 1 });
  expect(records.filter(record => matches(record, "other")).map(record => record.runId)).toEqual([3, 4]);
  expect(records.filter(record => matches(record, "all"))).toHaveLength(4);
  expect(records.filter(record => matches(record, "done")).map(record => record.runId)).toEqual([1]);
  expect(records.map(runSignal)).toEqual(["done", "idle", "attention", "working"]);
});

it("draws a clip wave only from the events that run actually recorded", () => {
  const now = Date.UTC(2026, 9, 15, 10, 0, 0);
  const [record] = runRecords(detail([turn({ run: run({ id: 1 }), node: node({ id: 1 }) })]));
  const event = (id: number, at: string, nodeRun: number): RunEvent => ({ id, node_run_id: nodeRun, kind: "note", actor: "pi", summary: "step", message: null, thinking: [], at });
  expect(clipActivity([event(1, "2026-10-15T09:01:00Z", 1)], record!, now)).toBeNull();
  expect(clipActivity([event(1, "2026-10-15T09:01:00Z", 2), event(2, "2026-10-15T09:02:00Z", 2)], record!, now)).toBeNull();
  expect(clipActivity([event(1, "2026-10-15T08:00:00Z", 1), event(2, "2026-10-15T09:40:00Z", 1)], record!, now)).toBeNull();
  const wave = clipActivity([event(1, "2026-10-15T09:01:00Z", 1), event(2, "2026-10-15T09:02:00Z", 1), event(3, "2026-10-15T09:29:00Z", 1)], record!, now, 4);
  expect(wave).toEqual([1, 0, 0, 0.5]);
  const [stopped] = runRecords(detail([turn({ run: run({ id: 1, status: "running", ended_at: null }), node: node({ id: 1, status: "running" }) })]));
  expect(clipActivity([event(1, "2026-10-15T09:01:00Z", 1), event(2, "2026-10-15T09:02:00Z", 1)], stopped!, now)).toBeNull();
});

it("reads a prompt's first line for a tooltip and marks a continuation", () => {
  const records = runRecords(detail([
    turn({ run: run({ id: 1, prompt: "continue" }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, prompt: "\nFix the parser\nthen push" }), node: node({ id: 2 }) }),
  ]));
  expect(records.map(record => record.continuation)).toEqual([true, false]);
  expect(firstLine(records[1]!.prompt)).toBe("Fix the parser");
  expect(firstLine("x".repeat(200), 10)).toBe("xxxxxxxxx…");
});
