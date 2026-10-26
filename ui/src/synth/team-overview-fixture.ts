// Explicitly synthetic execution and two-seat telemetry. No processes or API writes.
import type { ChatDetail } from "../chat-api";
import type { TeamBuild, TeamRun } from "../team-api";
import { fixtureOverview } from "./overview-fixture";

export function fixtureTeamOverview() {
  const source = fixtureOverview(), turn = source.turns[0]!, template = source.live.seats[0]!;
  const execution: TeamRun = { chat_id: 1, run_id: 1, control_node_id: 1, phase: "building", base_sha: "a".repeat(40), approved_revision: 4, reason: null, rev: 2, quiescent: false };
  turn.team = execution; turn.node.role = "orchestrator"; turn.node.status = turn.run.status = "running"; turn.run.ended_at = turn.node.ended_at = null;
  turn.members = ["backend", "frontend"].map((role, i) => ({ node: { ...turn.node, id: i + 2, role, status: i ? "running" : "done", slice_key: `S${i + 1}`, worktree_path: `/fixture/draft-${i + 1}` }, live_text: "Synthetic seat activity only" }));
  source.live.mode = "team"; source.live.nodes_read = source.live.nodes_total = 3;
  source.live.seats = turn.members.map((member, i) => ({ ...template, role: member.node.role, node_id: member.node.id, slice_key: member.node.slice_key, worktree: member.node.worktree_path,
    status: member.node.status, live: i === 1, supervised: i === 1 ? true : null, ended_at: i === 1 ? null : template.ended_at,
    usage: { tokens_in: template.usage.tokens_in * (i + 1), tokens_out: template.usage.tokens_out * (i + 1), cache_read: template.usage.cache_read * (i + 1), cache_write: template.usage.cache_write * (i + 1) },
    context_tokens: 700 * (i + 1) }));
  source.live.totals.usage = { tokens_in: 600, tokens_out: 135, cache_read: 4500, cache_write: 0 }; source.live.totals.turns = 3; source.live.totals.steps = 4;
  source.events = source.events.flatMap(event => [0, 1].map(i => {
    const copy = structuredClone(event); copy.node_run_id = i + 2; copy.session_id = `fixture-seat-${i}`;
    if (copy.observation) copy.observation.clock_id = `fixture-reader-${i}`;
    if (i && copy.telemetry?.kind === "usage") {
      for (const key of ["input", "output", "cache_read", "cache_write", "context"] as const) if (copy.telemetry.tokens[key] !== null) copy.telemetry.tokens[key] *= 2;
      if (copy.telemetry.sequence === 2 && copy.observation) { copy.observation.clock_id = "fixture-reader-restarted"; copy.session_id = "fixture-seat-restarted"; }
    }
    if (i && copy.telemetry?.kind === "compaction") {
      if (copy.telemetry.tokens_before !== null) copy.telemetry.tokens_before *= 2;
      if (copy.telemetry.estimated_tokens_after !== null) copy.telemetry.estimated_tokens_after *= 2;
    }
    return copy;
  })).map((event, index) => ({ ...event, id: index + 3 }));
  source.view.map.zones = [{ role: "backend", name: "Backend", zone: "src/**", owns: 1 }, { role: "frontend", name: "Frontend", zone: "README.md", owns: 1 }];
  source.view.map.nodes = source.view.map.nodes.map(node => ({ ...node, owner: node.owner === null ? null : node.path === "README.md" ? "frontend" : "backend" }));
  const build: TeamBuild = { execution, closure: null, slices: turn.members.map((member, i) => ({ run_id: 1, slice_key: member.node.slice_key!, worktree_path: member.node.worktree_path, branch: `fixture/S${i + 1}`, lease_state: i ? "leased" : "retained", build_status: i ? "building" : "verified", candidate_sha: i ? null : "b".repeat(40), commit_sha: i ? null : "b".repeat(40), release_started: false, reason: null, rev: 1 })) };
  const detail: ChatDetail = { id: 1, project_id: 1, mode: "team", title: "Synthetic team evidence", workspace_path: "/fixture/demo", workspace_epoch: 0, provider: "local", model: "fixture-model", reasoning: "high", active_node_id: 1, live_text: "", stop_requested: false, archived: false, rev: 1, created_at: turn.run.created_at, updated_at: turn.run.created_at, state: "running", can_resume: false, orphan_running: false, turns: source.turns, team_builds: [build], followups: [] };
  return { ...source, detail };
}
