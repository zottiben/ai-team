import type { RunEvent } from "../../api";
import type { Telemetry } from "./activity-api";

export const BUCKET_MS = 15_000;
export const BUCKETS = 64;
export const RECEIPTS = 4096;
export const SAMPLES = 120;
export type Measurement = RunEvent & { telemetry: Telemetry };
const measured = (event: RunEvent): event is Measurement => event.telemetry != null;
export const count = (value: unknown): number | null => typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;

/** One immutable chat feed, narrowed to an exact node, never the node's mutable session.
 * This bound is a display horizon, not permission to prune durable receipts. */
export function measurements(events: readonly RunEvent[], node: number): Measurement[] {
  const seen = new Set<number>();
  const found: Measurement[] = [];
  for (const event of events) {
    if (seen.has(event.id)) continue;
    seen.add(event.id);
    if (event.node_run_id === node && measured(event)) found.push(event);
  }
  return found.slice(-RECEIPTS);
}

function scope(event: Measurement): string | null {
  const session = event.session_id, clock = event.observation?.clock_id;
  return typeof session === "string" && session.length > 0 && typeof clock === "string" && clock.length > 0
    ? JSON.stringify([event.node_run_id, session, clock]) : null;
}
function series(event: Measurement): string | null {
  const key = scope(event), sequence = count(event.telemetry.sequence);
  return key !== null && sequence !== null ? JSON.stringify([key, sequence]) : null;
}
function stamp(event: Measurement): number | null {
  // Never backfill precision from event.at (ingest time, historically second resolution).
  const value = event.observation?.at ? Date.parse(event.observation.at) : NaN;
  return Number.isFinite(value) ? value : null;
}

/** Reported completed output / 15s wall bucket. Empty is unknown, not measured zero.
 * The last bucket is explicitly partial; this is not instantaneous generation speed. */
export function completedRates(events: readonly Measurement[], now: number): (number | null)[] {
  const values: (number | null)[] = Array.from({ length: BUCKETS }, () => null);
  if (!Number.isFinite(now)) return values;
  const last = Math.floor(now / BUCKET_MS);
  const invalid = new Set<number>();
  for (const event of events) {
    if (event.telemetry.kind !== "usage" || event.telemetry.phase !== "step") continue;
    const at = stamp(event), output = count(event.telemetry.tokens.output);
    if (at === null || at > now) continue;
    const index = Math.floor(at / BUCKET_MS) - last + BUCKETS - 1;
    if (index < 0 || index >= BUCKETS || invalid.has(index)) continue;
    if (output === null) { values[index] = null; invalid.add(index); continue; }
    const total = (values[index] ?? 0) + output;
    if (!Number.isSafeInteger(total)) { values[index] = null; invalid.add(index); }
    else values[index] = total;
  }
  return values.map(value => value === null ? null : value / (BUCKET_MS / 1000));
}

/** Differences of cumulative counters over same-clock receive intervals, not execution
 * throughput. A broken counter series remains invalid until a new host sequence. */
export function observedRates(events: readonly Measurement[]): (number | null)[] {
  const previous = new Map<string, { output: number; elapsed: number }>();
  const broken = new Set<string>();
  const rates: (number | null)[] = [];
  let lastSeries: string | null = null;
  for (const event of events) {
    if (event.telemetry.kind !== "usage" || event.telemetry.phase === "step") continue;
    const key = series(event), output = count(event.telemetry.tokens.output), elapsed = count(event.observation?.elapsed_ms);
    const prior = key === null ? undefined : previous.get(key);
    let rate: number | null = null;
    if (key !== null && output !== null && elapsed !== null && !broken.has(key)) {
      if (prior && (output < prior.output || elapsed < prior.elapsed)) broken.add(key);
      else if (prior && elapsed > prior.elapsed) rate = (output - prior.output) * 1000 / (elapsed - prior.elapsed);
      previous.set(key, { output, elapsed });
    } else if (key !== null) previous.delete(key);
    if (key !== lastSeries) rates.push(null);
    rates.push(rate);
    lastSeries = key;
  }
  return rates.slice(-SAMPLES);
}

export type ContextTrace = { values: (number | null)[]; markers: number[]; latest: number | null };
/** Sample-position axis, not elapsed time. Process/session changes and missing counts
 * break the line. Estimated post-compaction counts never replace observed context. */
export function contextTrace(events: readonly Measurement[]): ContextTrace {
  const values: (number | null)[] = [], markers: number[] = [];
  const starts = new Map<string, { reason: string | null; duplicate: boolean }>();
  let priorScope: string | null = null, latest: number | null = null;
  for (const event of events) {
    const metric = event.telemetry;
    if (metric.kind === "compaction") {
      const key = series(event);
      if (key === null) continue;
      if (metric.phase === "started") starts.set(key, { reason: metric.reason, duplicate: starts.has(key) });
      else {
        const start = starts.get(key);
        if (metric.phase === "completed" && start && !start.duplicate && start.reason === metric.reason && values.length > 0 && scope(event) === priorScope) markers.push(values.length - 1);
        starts.delete(key);
      }
      continue;
    }
    const currentScope = scope(event);
    if (currentScope === null || currentScope !== priorScope) values.push(null);
    latest = count(metric.tokens.context);
    values.push(latest);
    priorScope = currentScope;
  }
  const offset = Math.max(0, values.length - SAMPLES);
  return { values: values.slice(offset), markers: markers.filter(index => index >= offset).map(index => index - offset), latest };
}
