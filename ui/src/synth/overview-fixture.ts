// Synthetic telemetry and checkout evidence. Imported only by the isolated Vite fixture.
import type { RunEvent } from "../api";
import type { ChatMapView, ChatOverview } from "../chat-overview-api";
import type { Telemetry } from "./data/activity-api";
import { fixtureRuns } from "./work-fixture";

export function fixtureOverview() {
  const base = Date.now() - 14_000, at = (offset: number) => new Date(base + offset).toISOString();
  const turns = fixtureRuns().slice(0, 1);
  for (const turn of turns) {
    turn.run.started_at = turn.node.started_at = at(0);
    turn.run.ended_at = turn.node.ended_at = at(10_000);
  }
  const events: RunEvent[] = [];
  const receipt = (offset: number, telemetry: Telemetry) => events.push({ id: events.length + 3, node_run_id: 1, actor: "assistant", kind: telemetry.kind === "usage" && telemetry.phase === "step" ? "cost" : "note",
    summary: "Synthetic telemetry receipt", message: null, thinking: [], at: at(offset), session_id: "fixture-session", telemetry_only: !(telemetry.kind === "usage" && telemetry.phase === "step"),
    observation: { clock_id: "fixture-reader", elapsed_ms: offset, at: at(offset) }, telemetry });
  const usage = (offset: number, phase: "live" | "message_end" | "step", output: number, sequence = 1, context = 1000) => receipt(offset,
    { kind: "usage", phase, sequence, tokens: { input: 100, output, cache_read: context - 100, cache_write: 0, context, context_source: "complete_parts" } });
  usage(1000, "live", 10); usage(2000, "live", 20); usage(2500, "message_end", 30); usage(3000, "step", 30);
  receipt(4000, { kind: "compaction", phase: "started", reason: "manual", sequence: 1, tokens_before: null, estimated_tokens_after: null });
  receipt(4500, { kind: "compaction", phase: "completed", reason: "manual", sequence: 1, tokens_before: 1000, estimated_tokens_after: 500 });
  usage(5000, "live", 0, 2, 700); usage(6000, "message_end", 15, 2, 700); usage(7000, "step", 15, 2, 700);
  const counts = { tokens_in: 200, tokens_out: 45, cache_read: 1500, cache_write: 0 };
  const live: ChatOverview = { chat_id: 1, project: "Demo project · fixture", workspace: "/fixture/demo", mode: "single", nodes_total: 1, nodes_read: 1,
    totals: { turns: 1, steps: 2, usage: counts, files_touched: 2, files_written: 1 },
    seats: [{ role: "assistant", node_id: 1, run_id: 1, provider: "local", model: "fixture-model", status: "done", attempt: 1, slice_key: null, worktree: "/fixture/demo", started_at: at(0), ended_at: at(10_000), blocked_reason: null,
      live: false, supervised: null, activity: { kind: "tool_result", summary: "Synthetic edit result", detail: "src/lib.rs", file: "src/lib.rs", at: at(8000) }, said: "Synthetic completed work — no agent was invoked.", runs: 1, steps: 2, usage: counts, context_tokens: 700, outside: 0,
      touches: [{ path: "src/lib.rs", tool: "edit", reads: 1, writes: 2, at: at(8000) }, { path: "README.md", tool: "read", reads: 1, writes: 0, at: at(500) }], live_touches: [] }] };
  const view: ChatMapView = { workspace: "/fixture/demo", map: { root: "demo", files: 3, unowned: 1, truncated: false,
    zones: [{ role: "assistant", name: "Assistant", zone: "src/**, README.md", owns: 2 }],
    nodes: [{ path: "", name: "demo", dir: true, depth: 0, weight: 3, owner: null }, { path: "src", name: "src", dir: true, depth: 1, weight: 1, owner: "assistant" },
      { path: "src/lib.rs", name: "lib.rs", dir: false, depth: 2, weight: 1, owner: "assistant" }, { path: "README.md", name: "README.md", dir: false, depth: 1, weight: 1, owner: "assistant" }, { path: "notes.txt", name: "notes.txt", dir: false, depth: 1, weight: 1, owner: null }],
    edges: [{ from: 0, to: 1 }, { from: 1, to: 2 }, { from: 0, to: 3 }, { from: 0, to: 4 }] } };
  return { turns, events, live, view };
}
