import { post, type NodeRun } from "./api";
import type { Chat } from "./chat-api";
import type { ChatPlan } from "./plan-api";

export type TeamRun = {
  run_id: number;
  chat_id: number;
  control_node_id: number;
  phase: "grounding" | "planning" | "awaiting_approval" | "building" | "blocked" | "finished";
  base_sha: string | null;
  approved_revision: number | null;
  reason: string | null;
  rev: number;
  quiescent: boolean;
};
export type TeamTarget = {
  chat_id: number;
  run_id: number;
  node_id: number;
  expect_revision: number;
};
export const teamTarget = (run: TeamRun): TeamTarget => ({
  chat_id: run.chat_id, run_id: run.run_id, node_id: run.control_node_id, expect_revision: run.rev,
});
export type TeamMember = { node: NodeRun & { started_at: string | null }; live_text: string };
export type BuildSlice = {
  run_id: number;
  slice_key: string;
  worktree_path: string | null;
  branch: string | null;
  lease_state: string;
  build_status: string;
  candidate_sha: string | null;
  commit_sha: string | null;
  release_started: boolean;
  reason: string | null;
  rev: number;
};
export type TeamBuild = {
  execution: TeamRun;
  slices: BuildSlice[];
  closure: { reason: string; finished_at: string | null; issues: string[] } | null;
};
export type TeamRecovery = {
  target: TeamTarget;
  state: "active" | "quiescent" | "pending_dispatch" | "recoverable" | "recovered" | "needs_inspection";
  reason: string | null;
};
export type BuildReview = {
  execution: TeamRun;
  plan: ChatPlan;
  roster: { id: number; role: string; provider: string; model: string; enabled: boolean; read_only: boolean; zone: string }[];
  roster_revision: string;
  head: string;
  dirty: string;
};
export function setChatMode(id: number, mode: Chat["mode"], revision: number): Promise<Chat> {
  return post(`/chats/${id}/mode`, { mode, expect_revision: revision });
}
export function reviewTeam(run: TeamRun): Promise<BuildReview> {
  return post(`/chats/${run.chat_id}/team/review`, teamTarget(run));
}
export function approveTeam(review: BuildReview): Promise<unknown> {
  return post(`/chats/${review.execution.chat_id}/team/approve`, {
    target: teamTarget(review.execution),
    approval: {
      expect_control_revision: review.execution.rev,
      expect_plan_revision: review.plan.revision,
      expect_roster_revision: review.roster_revision,
      expect_head: review.head,
    },
  });
}
export function teamCommand(run: TeamRun, action: "stop" | "recover" | "reconcile"): Promise<unknown> {
  return post(`/chats/${run.chat_id}/team/${action}`, teamTarget(run));
}
export function continueTeam(run: TeamRun, slice: BuildSlice): Promise<unknown> {
  return post(`/chats/${run.chat_id}/team/continue`, {
    target: teamTarget(run), slice_key: slice.slice_key, expect_slice_revision: slice.rev,
  });
}
export function closeTeam(build: TeamBuild, planRevision: number, reason: string): Promise<unknown> {
  return post(`/chats/${build.execution.chat_id}/team/close`, {
    target: teamTarget(build.execution), expect_plan_revision: planRevision,
    expect_slices: Object.fromEntries(build.slices.map((slice) => [slice.slice_key, slice.rev])), reason,
  });
}
