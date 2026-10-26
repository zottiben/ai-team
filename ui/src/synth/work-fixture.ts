// Synthetic Work evidence for the transport-isolated shell, never production telemetry.
import type { ChatTurn } from "../chat-api";

export function fixtureRuns(): ChatTurn[] {
  return ["done", "cancelled", "failed", "blocked", "queued", "running"].map((status, index) => ({
    run: { id: index + 1, project_id: 1, prompt: `Fixture run ${index + 1}: ${status}.\n${"This is synthetic layout evidence, not a real execution. ".repeat(8)}`,
      status, trigger: "manual", workspace_path: "/fixture/demo", created_at: "2026-01-01T00:00:00Z",
      started_at: "2026-01-01T00:00:00Z", ended_at: index < 4 ? "2026-01-01T00:10:00Z" : null },
    node: { id: index + 1, role: "assistant", provider: "local", model: "fixture-model", status, attempt: 1,
      slice_key: null, worktree_path: "/fixture/demo", branch: null, blocked_reason: status === "blocked" ? "Fixture blocker" : null,
      started_at: "2026-01-01T00:00:00Z", ended_at: null, usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 } },
    team: null, members: [],
  }));
}
