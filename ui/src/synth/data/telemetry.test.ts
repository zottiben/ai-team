import { expect, it } from "vitest";
import type { RunEvent } from "../../api";
import { BUCKET_MS, completedRates, contextTrace, measurements, observedRates, RECEIPTS, type Measurement } from "./telemetry";

const BASE = Date.UTC(2026, 0, 1);
function usage(id: number, elapsed: number, output: number | null, phase: "live" | "message_end" | "step" = "live", extra: Partial<RunEvent> = {}): Measurement {
  return { id, node_run_id: 1, session_id: "session", kind: "note", actor: "assistant", summary: "receipt", message: null, thinking: [], at: "2026-01-01",
    observation: { clock_id: "clock", elapsed_ms: elapsed, at: new Date(BASE + elapsed).toISOString() }, ...extra,
    telemetry: { kind: "usage", phase, sequence: 1, tokens: { input: 100, output, cache_read: 20, cache_write: 0, context: 120, context_source: "complete_parts" } } };
}
function compaction(id: number, phase: "started" | "completed" | "aborted"): Measurement {
  return { ...usage(id, id * 1000, null), telemetry: { kind: "compaction", phase, sequence: 1, reason: "threshold", tokens_before: 120, estimated_tokens_after: 50 } };
}

it("bounds the exact-node view and never replaces the first immutable receipt on replay", () => {
  const first = usage(1, 0, 1);
  const found = measurements([first, { ...first, node_run_id: 2 }, usage(2, 1, 2, "live", { node_run_id: 2 })], 1);
  expect(found).toEqual([first]);
  expect(measurements([first, { ...first, node_run_id: 2 }], 2)).toEqual([]);
  expect(measurements(Array.from({ length: RECEIPTS + 100 }, (_, i) => usage(i, i, i)), 1)).toHaveLength(RECEIPTS);
});

it("buckets only canonical completions, leaves empty intervals unknown, and preserves actual zero", () => {
  const live = usage(1, 1000, 150), end = usage(2, 2000, 150, "message_end"), step = usage(3, 3000, 150, "step");
  const zero = usage(4, BUCKET_MS + 1, 0, "step");
  const rates = completedRates(measurements([live, end, step, step, zero], 1), BASE + BUCKET_MS * 2);
  expect(rates).toHaveLength(64);
  expect(rates.slice(-3)).toEqual([10, 0, null]);
  expect(rates.slice(0, -3).every(value => value === null)).toBe(true);
});

it("does not use coarse ingestion dates or count future or invalid completion evidence", () => {
  const absent = usage(1, 0, 100, "step", { observation: null });
  const future = usage(2, BUCKET_MS * 2, 100, "step");
  expect(completedRates([absent, future], BASE + BUCKET_MS).every(value => value === null)).toBe(true);
  const valid = usage(3, 0, 15, "step"), invalid = usage(4, 1, null, "step");
  expect(completedRates([valid, invalid], BASE + 2).at(-1)).toBeNull();
  expect(completedRates([usage(5, 0, Number.MAX_SAFE_INTEGER, "step"), usage(6, 1, 1, "step")], BASE + 2).at(-1)).toBeNull();
});

it("derives only same-message receive-interval rates, never an execution duration", () => {
  expect(observedRates([usage(1, 0, 10), usage(2, 1000, 30), usage(3, 2000, 30, "message_end"), usage(4, 3000, 30, "step")])).toEqual([null, null, 20, 0]);
  const restarted = usage(3, 2000, 60, "live", { observation: { clock_id: "new-clock", elapsed_ms: 2000, at: new Date(BASE + 2000).toISOString() } });
  expect(observedRates([usage(1, 0, 10), usage(2, 1000, 30), restarted]).at(-1)).toBeNull();
  expect(observedRates([usage(1, 0, 10), usage(2, 1000, 30, "live", { session_id: "different" })]).at(-1)).toBeNull();
  const unknown = usage(3, 1000, 30); unknown.telemetry.sequence = null;
  expect(observedRates([usage(1, 0, 10), unknown]).at(-1)).toBeNull();
});

it("poisons a regressing series and breaks gaps rather than inventing catch-up rates", () => {
  const points = [usage(1, 0, 10), usage(2, 1000, 5), usage(3, 2000, 20)];
  expect(observedRates(points).every(value => value === null)).toBe(true);
  const recovered = usage(4, 3000, 30); recovered.telemetry.sequence = 2;
  const next = usage(5, 4000, 50); next.telemetry.sequence = 2;
  expect(observedRates([...points, recovered, next]).at(-1)).toBe(20);
  expect(observedRates([usage(1, 0, 10), usage(2, 1000, null), usage(3, 2000, 20)]).at(-1)).toBeNull();
  expect(observedRates([usage(1, 0, 10), usage(2, 0, 30)]).at(-1)).toBeNull();
  expect(observedRates([usage(1, 0, 10), usage(2, 1000, -1)]).at(-1)).toBeNull();
});

it("requires an explicit matched successful compaction and never plots its estimated post count as context", () => {
  const trace = contextTrace([usage(1, 0, 10), compaction(2, "started"), compaction(3, "completed"), usage(4, 4000, 20)]);
  expect(trace).toEqual({ values: [null, 120, 120], markers: [1], latest: 120 });
  expect(contextTrace([usage(1, 0, 10), compaction(2, "completed")]).markers).toEqual([]);
  expect(contextTrace([usage(1, 0, 10), compaction(2, "started"), compaction(3, "aborted")]).markers).toEqual([]);
  expect(contextTrace([usage(1, 0, 10), compaction(2, "started"), compaction(3, "started"), compaction(4, "completed")]).markers).toEqual([]);
});

it("keeps context gaps across sessions and scopes all chart state to bounded history", () => {
  const trace = contextTrace([usage(1, 0, 10), usage(2, 1000, 20, "live", { session_id: "next" }), usage(3, 2000, 30, "live", { observation: null })]);
  expect(trace.values).toEqual([null, 120, null, 120, null, 120]);
  const history = Array.from({ length: 200 }, (_, i) => usage(i, i * 1000, i));
  expect(contextTrace(history).values).toHaveLength(120);
  expect(observedRates(history)).toHaveLength(120);
});
