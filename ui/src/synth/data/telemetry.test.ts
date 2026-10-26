import { expect, it } from "vitest";
import type { RunEvent } from "../../api";
import { BUCKET_MS, compactionEvidence, completedRates, contextTrace, measurements, observedRates, RECEIPTS, tokenTrace, type Measurement } from "./telemetry";

const BASE = Date.UTC(2026, 0, 1);
function usage(id: number, elapsed: number, output: number | null, phase: "live" | "message_end" | "step" = "live", extra: Partial<RunEvent> = {}): Measurement {
  return { id, node_run_id: 1, session_id: "session", kind: "note", actor: "assistant", summary: "receipt", message: null, thinking: [], at: "2026-01-01",
    observation: { clock_id: "clock", elapsed_ms: elapsed, at: new Date(BASE + elapsed).toISOString() }, ...extra,
    telemetry: { kind: "usage", phase, sequence: 1, tokens: { input: 100, output, cache_read: 20, cache_write: 0, context: 120, context_source: "complete_parts" } } };
}
function compaction(id: number, phase: "started" | "completed" | "aborted"): Measurement {
  return { ...usage(id, id * 1000, null), telemetry: { kind: "compaction", phase, sequence: 1, reason: "threshold", tokens_before: 120, estimated_tokens_after: 50 } };
}

it("plots live input, output and context without adding observations to accounting totals", () => {
  const trace = tokenTrace([usage(1, 0, 10), usage(2, 1000, 30), usage(3, 2000, 30, "step")]);
  expect(trace.input).toEqual([100, 100, 100]);
  expect(trace.output).toEqual([10, 30, 30]);
  expect(trace.context).toEqual([120, 120, 120]);
  expect(trace.at).toBe(new Date(BASE + 2000).toISOString());
});

it("leaves initial live zeros and invalid counts unknown but preserves completed zero", () => {
  const zero = usage(1, 0, 0);
  if (zero.telemetry.kind !== "usage") throw Error("Expected usage fixture");
  zero.telemetry.tokens.input = 0; zero.telemetry.tokens.context = 0;
  const invalid = usage(3, 2000, -1);
  if (invalid.telemetry.kind !== "usage") throw Error("Expected usage fixture");
  invalid.telemetry.tokens.input = Number.MAX_SAFE_INTEGER + 1; invalid.telemetry.tokens.context = NaN;
  const end = { ...zero, id: 2, telemetry: { ...zero.telemetry, phase: "message_end" as const } };
  const trace = tokenTrace([zero, end, invalid]);
  expect(trace.input).toEqual([null, 0, null]);
  expect(trace.output).toEqual([null, 0, null]);
  expect(trace.context).toEqual([null, 0, null]);
});

it("exposes reported cache reads and writes without estimating missing values or adding totals", () => {
  const live = usage(1, 0, 10), end = usage(2, 1000, 20, "step"), missing = usage(3, 2000, 30);
  if (live.telemetry.kind !== "usage" || end.telemetry.kind !== "usage" || missing.telemetry.kind !== "usage") throw Error("Expected usage fixtures");
  missing.telemetry.tokens.cache_read = null; missing.telemetry.tokens.cache_write = -1;
  const trace = tokenTrace([live, end, missing], "levels");
  expect(trace.cache_read).toEqual([20, 20, null]);
  expect(trace.cache_write).toEqual([null, 0, null]);
  expect(live.telemetry.tokens.cache_read).toBe(20);
});

it("breaks live scope lines at response, session and process boundaries", () => {
  const nextResponse = usage(2, 1000, 20); nextResponse.telemetry.sequence = 2;
  for (const next of [nextResponse, usage(2, 1000, 20, "live", { session_id: "next" }), usage(2, 1000, 20, "live", { observation: null }),
    usage(2, 1000, 20, "live", { observation: { clock_id: "next-reader", elapsed_ms: 1000, at: new Date(BASE + 1000).toISOString() } })]) {
    expect(tokenTrace([usage(1, 0, 10), next]).output).toEqual([10, null, 20]);
  }
  expect(tokenTrace([usage(1, 0, 10, "live", { observation: null })]).at).toBeNull();
});

it("connects reported response levels without summing them, but retains unknown reader and compaction gaps", () => {
 const next=usage(2,1000,5);next.telemetry.sequence=2;
 expect(tokenTrace([usage(1,0,40),next], "levels").output).toEqual([40,5]);
 expect(tokenTrace([usage(1,0,40),{...next,session_id:"other"}], "levels").output).toEqual([40,null,5]);
 expect(tokenTrace([usage(1,0,40),compaction(3,"completed"),next], "levels").output).toContain(null);
 expect(tokenTrace([usage(1,0,40),usage(2,1,0)], "levels").output).toEqual([40,null]);
 next.telemetry.sequence=null;expect(tokenTrace([usage(1,0,40),next], "levels").output).toEqual([40,5]);
});

it("does not draw compaction estimates as live context and bounds every channel", () => {
  const trace = tokenTrace([usage(1, 0, 10), compaction(2, "started"), compaction(3, "completed")]);
  expect(trace.context.at(-1)).toBeNull();
  expect(trace.context).not.toContain(50);
  expect(trace.at).toBeNull();
  const bounded = tokenTrace(Array.from({ length: RECEIPTS + 100 }, (_, i) => usage(i, i, i + 1)));
  for (const values of [bounded.input, bounded.output, bounded.context, bounded.cache_read, bounded.cache_write]) expect(values).toHaveLength(120);
  expect(tokenTrace([])).toEqual({ input: [], output: [], context: [], cache_read: [], cache_write: [], at: null });
});

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
  const trace = contextTrace([usage(1, 0, 10, "step"), compaction(2, "started"), compaction(3, "completed"), usage(4, 4000, 20, "step")]);
  expect(trace).toEqual({ values: [120, 120], markers: [0], latest: 120 });
  expect(contextTrace([usage(1, 0, 10, "step"), compaction(2, "completed")]).markers).toEqual([]);
  expect(contextTrace([usage(1, 0, 10, "step"), compaction(2, "started"), compaction(3, "aborted")]).markers).toEqual([]);
  expect(contextTrace([usage(1, 0, 10, "step"), compaction(2, "started"), compaction(3, "started"), compaction(4, "completed")]).markers).toEqual([]);
});

it("discloses missing identities, orphan starts/ends, matched aborts and conflicting receipts", () => {
  const start = compaction(1, "started"), end = compaction(2, "completed");
  const labels = (events: Measurement[]) => compactionEvidence(events).map(row => row.correlation);
  expect(labels([start])).toEqual(["start_only"]);
  expect(labels([end])).toEqual(["end_only"]);
  expect(labels([start, end])).toEqual(["paired", "paired"]);
  expect(labels([start, compaction(3, "aborted")])).toEqual(["paired", "paired"]);
  expect(labels([end, start])).toEqual(["ambiguous", "ambiguous"]);
  expect(labels([start, { ...end, session_id: "another" }])).toEqual(["start_only", "end_only"]);
  expect(labels([{ ...end, observation: null }])).toEqual(["missing_identity"]);
});

it("does not put an ambiguous duplicate compaction end on the context plot", () => {
  expect(contextTrace([usage(1, 0, 10, "step"), compaction(2, "started"), compaction(3, "completed"), compaction(4, "completed")]).markers).toEqual([]);
});

it("keeps context gaps across sessions and scopes all chart state to bounded history", () => {
  const trace = contextTrace([usage(1, 0, 10, "step"), usage(2, 1000, 20, "step", { session_id: "next" }), usage(3, 2000, 30, "step", { observation: null })]);
  expect(trace.values).toEqual([120, null, 120, null, 120]);
  expect(contextTrace(Array.from({ length: 200 }, (_, i) => usage(i, i * 1000, i, "step"))).values).toHaveLength(120);
  expect(observedRates(Array.from({ length: 200 }, (_, i) => usage(i, i * 1000, i)))).toHaveLength(120);
  expect(contextTrace([usage(1, 0, 10), usage(2, 1000, 20, "message_end")])).toEqual({ values: [], markers: [], latest: 120 });
});
