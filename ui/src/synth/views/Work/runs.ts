import type { RunEvent } from "../../../api";
import type { ChatDetail } from "../../../chat-api";
import { TEAM_PHASES } from "../../../team-api";
import type { Signal } from "../../primitives/signal";

/**
 * What the Work panel is allowed to say about a run.
 *
 * Every field here is read from a stored row. The panel adds decoration, never a state:
 * a turn that stopped without an end stamp has an unknown duration, not a zero one, and
 * an interrupted turn is not a live one just because its row still says `running`.
 */
export type RunKind = "done" | "cancelled" | "live" | "other";
export type RunRecord = {
  runId: number;
  nodeId: number;
  prompt: string;
  /** `continue` is the one prompt the spec renders differently (§9.2). */
  continuation: boolean;
  provider: string;
  model: string;
  attempt: number;
  reasoning: string | null;
  sliceKey: string | null;
  blockedReason: string | null;
  kind: RunKind;
  /** The stored vocabulary, for grouping and for `data-status`. Never a label. */
  status: string;
  label: string;
  live: boolean;
  /** Wall start and end, in epoch milliseconds. `null` means the stamp is missing. */
  start: number | null;
  end: number | null;
};

/**
 * Stamps arrive as RFC3339 UTC. Rows written by older paths carry SQLite's
 * space-separated UTC with no zone, and reading those as local time would move a run by
 * hours. Anything unparseable stays unknown.
 */
export function instant(value: string | null | undefined): number | null {
  if (!value) return null;
  const zoned = /([zZ]|[+-]\d{2}:?\d{2})$/.test(value);
  const parsed = Date.parse(zoned ? value : `${value.replace(" ", "T")}Z`);
  return Number.isFinite(parsed) ? parsed : null;
}

/** The chat states that mean a process is running now. Interrupted is not one of them. */
const LIVE_STATES: ReadonlySet<string> = new Set(["running", "stopping"]);
const CHAT_STATE_LABELS: Record<ChatDetail["state"], string> = {
  empty: "Ready", idle: "Ready", running: "Working", stopping: "Stopping…",
  interrupted: "Interrupted", failed: "Needs attention", stopped: "Stopped",
  awaiting_approval: "Awaiting your approval", team_blocked: "Team needs attention",
  team_interrupted: "Team needs recovery",
};
const STORED_LABELS: Record<string, string> = {
  queued: "Queued", planning: "Planning", running: "Running", parked: "Parked",
  blocked: "Blocked", done: "Done", failed: "Failed", cancelled: "Cancelled",
};

export function runRecords(detail: ChatDetail): RunRecord[] {
  return detail.turns.map(turn => {
    const active = turn.node.id === detail.active_node_id;
    const stored = turn.team ? turn.run.status : turn.node.status;
    const live = active && LIVE_STATES.has(detail.state);
    const phase = turn.team ? TEAM_PHASES[turn.team.phase] : null;
    const label = active && (detail.state === "interrupted" || detail.state === "stopping" || detail.state === "team_interrupted")
      ? CHAT_STATE_LABELS[detail.state]
      : phase ?? (active ? CHAT_STATE_LABELS[detail.state] : null) ?? STORED_LABELS[stored] ?? stored;
    return {
      runId: turn.run.id,
      nodeId: turn.node.id,
      prompt: turn.run.prompt,
      continuation: turn.run.prompt.trim().toLowerCase() === "continue",
      provider: turn.node.provider,
      model: turn.node.model,
      attempt: turn.node.attempt,
      reasoning: turn.node.reasoning ?? null,
      sliceKey: turn.node.slice_key,
      blockedReason: turn.node.blocked_reason,
      kind: live ? "live" : stored === "done" ? "done" : stored === "cancelled" ? "cancelled" : "other",
      // The chat's own state is the sharper word for the turn that owns the chat: it
      // separates stopping and interrupted from the `running` both of them store.
      status: active ? detail.state : stored,
      label,
      live,
      // A node may finish before its enclosing run. Never mix their clock boundaries.
      start: instant(turn.run.started_at),
      end: instant(turn.run.ended_at),
    };
  });
}

/**
 * Wall duration: start to end, the measure the arrangement is labelled with.
 *
 * Not active runtime, which nothing records per run. A live turn is measured to now; a
 * turn that stopped without an end stamp is unknown, because inventing an end would make
 * a dead run grow for ever and a missing stamp look like a fast run.
 */
export function durationMs(record: RunRecord, now: number): number | null {
  if (record.start === null) return null;
  const end = record.end ?? (record.live ? now : null);
  return end === null || !Number.isFinite(end) || end < record.start ? null : end - record.start;
}

export function durationLabel(ms: number | null): string {
  if (ms === null) return "duration unknown";
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? `${minutes}m ${seconds % 60}s` : `${minutes}m`;
  return minutes % 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${Math.floor(minutes / 60)}h`;
}

export type Tick = { ms: number; label: string; fraction: number };
export type ArrangementScale = { proportional: boolean; totalMs: number; ticks: Tick[] };

const STEPS = [1, 5, 10, 30, 60, 120, 360, 720, 1440].map(minutes => minutes * 60_000);

/** 30-minute marks where they fit (§9.1), a finer step for a lane shorter than one. */
export function ruler(totalMs: number): Tick[] {
  if (!(totalMs > 0) || !Number.isSafeInteger(totalMs)) return [];
  const step = STEPS.find(candidate => totalMs / candidate <= 6) ?? Math.ceil(totalMs / 6 / 86_400_000) * 86_400_000;
  const ticks: Tick[] = [];
  for (let ms = 0; ms <= totalMs; ms += step) ticks.push({ ms, label: tickLabel(ms, step), fraction: ms / totalMs });
  return ticks;
}
/** Minutes while the marks are minutes apart, so a 30-minute ruler reads 0m 30m 60m 90m. */
function tickLabel(ms: number, step: number): string {
  const minutes = Math.round(ms / 60_000);
  if (step < 3_600_000) return `${minutes}m`;
  return minutes % 60 ? `${Math.floor(minutes / 60)}h${minutes % 60}` : `${Math.floor(minutes / 60)}h`;
}

/**
 * Clips are proportional only when every run can be measured. One unknown duration makes
 * the whole lane a comparison of guesses, so the spec's fallback applies instead: equal
 * widths and no ruler. A measured zero is a measurement and keeps its place.
 */
export function arrangementScale(durations: readonly (number | null)[]): ArrangementScale {
  const measured = durations.length > 0 && durations.every(value => value !== null && Number.isFinite(value) && value >= 0);
  const totalMs = measured ? durations.reduce((sum: number, value) => sum + (value ?? 0), 0) : 0;
  const proportional = measured && totalMs > 0;
  return { proportional, totalMs, ticks: proportional ? ruler(totalMs) : [] };
}

export type Counters = {
  runs: number; done: number; cancelled: number; live: number;
  others: { status: string; label: string; count: number }[];
};

/** Done, Cancelled and Live are the spec's counters. Everything else is counted too. */
export function counters(records: readonly RunRecord[]): Counters {
  const others = new Map<string, { status: string; label: string; count: number }>();
  for (const record of records) {
    if (record.kind !== "other") continue;
    const entry = others.get(record.status) ?? { status: record.status, label: record.label, count: 0 };
    entry.count += 1;
    others.set(record.status, entry);
  }
  return {
    runs: records.length,
    done: records.filter(record => record.kind === "done").length,
    cancelled: records.filter(record => record.kind === "cancelled").length,
    live: records.filter(record => record.kind === "live").length,
    others: [...others.values()],
  };
}

export type RunFilter = "all" | "done" | "cancelled" | "other";
/** `other` holds every state the two named filters would otherwise hide, live included. */
export function matches(record: RunRecord, filter: RunFilter): boolean {
  if (filter === "all") return true;
  if (filter === "other") return record.kind !== "done" && record.kind !== "cancelled";
  return record.kind === filter;
}

export function runSignal(record: RunRecord): Signal {
  if (record.live) return "working";
  if (record.kind === "done") return "done";
  if (record.kind === "cancelled") return "idle";
  return ["failed", "blocked", "team_blocked", "interrupted", "team_interrupted"].includes(record.status) ? "attention" : "info";
}

/** The first line of a prompt, for a tooltip that must stay one line. */
export function firstLine(prompt: string, limit = 80): string {
  const line = prompt.split("\n").find(text => text.trim().length > 0)?.trim() ?? "";
  return line.length > limit ? `${line.slice(0, limit - 1)}…` : line;
}

/**
 * A clip's mini-wave, from the events the run actually recorded.
 *
 * `null` when the window is unknown or the run recorded too little to describe, which the
 * clip renders as a flat fill. An illustrative trace would be a picture of work that was
 * never done.
 */
export function clipActivity(events: readonly RunEvent[], record: RunRecord, now: number, buckets = 12): number[] | null {
  const start = record.start;
  const end = record.end ?? (record.live ? now : null);
  if (start === null || end === null || end <= start) return null;
  const counts = new Array<number>(buckets).fill(0);
  let total = 0;
  for (const event of events) {
    if (event.telemetry_only || event.node_run_id !== record.nodeId) continue;
    const at = instant(event.at);
    if (at === null || at < start || at > end) continue;
    const bucket = Math.min(buckets - 1, Math.floor(((at - start) / (end - start)) * buckets));
    counts[bucket] = (counts[bucket] ?? 0) + 1;
    total += 1;
  }
  if (total < 2) return null;
  const peak = Math.max(...counts);
  return counts.map(count => count / peak);
}
