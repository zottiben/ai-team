import { api, post } from "./api";

export const PLAN_STATUSES = [
  "draft",
  "ready",
  "active",
  "in_review",
  "blocked",
  "done",
  "deferred",
] as const;
export type PlanStatus = (typeof PLAN_STATUSES)[number];
export type PlanSlice = {
  id: number;
  key: string;
  title: string;
  status: PlanStatus;
  scope_md: string;
  demo_md: string | null;
  blocked_reason: string | null;
  branch?: string | null;
  worktree_path?: string | null;
  pr_url?: string | null;
  estimate_files?: number | null;
  claimed_by?: string | null;
  rev: number;
};
export type PlanSection = {
  key: string;
  title: string;
  body: string;
  rev: number;
};
export type PlanQuestion = {
  id: number;
  body: string;
  status: string;
  answer: string | null;
  slice_key: string | null;
};
export type ChatPlan = {
  archived?: boolean;
  frozen?: boolean;
  chat_id: number;
  project_id: number;
  revision: number;
  bundle: null | {
    plan: {
      id: number;
      slug: string;
      title: string;
      summary: string | null;
      status: PlanStatus;
    };
    sections: PlanSection[];
    slices: PlanSlice[];
    questions: PlanQuestion[];
    decisions: {
      id: number;
      key: string;
      title: string;
      body: string;
      status: string;
    }[];
    gotchas: { id: number; title: string; body: string }[];
    log: { id: number; body: string; actor: string | null; at: string }[];
  };
};
export type PlanAction =
  | { action: "create_plan"; title: string; summary?: string }
  | { action: "set_plan_status"; status: PlanStatus }
  | { action: "write_section"; key: string; title: string; body: string }
  | {
      action: "add_slice" | "update_slice";
      key: string;
      title: string;
      scope: string;
      touches: string[];
      demo: string;
    }
  | {
      action: "set_slice_status";
      key: string;
      status: PlanStatus;
      reason?: string;
    }
  | { action: "open_question"; body: string; slice?: string }
  | { action: "answer_question"; question_id: number; answer: string };

export function chatPlan(id: number): Promise<ChatPlan> {
  return api(`/chats/${id}/plan`);
}
export function changeChatPlan(
  id: number,
  action: PlanAction,
  revision: number,
): Promise<ChatPlan> {
  return post(`/chats/${id}/plan`, { ...action, expect_revision: revision });
}
