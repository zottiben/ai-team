import { api, post, type FileDiff, type RunEvent } from "./api";

export type DraftTarget = { run_id: number; slice_key: string; revision: number };
export type Draft = {
  target: DraftTarget; base_sha: string; commit_sha: string; branch: string;
  lease_state: string; worktree_path: string | null;
};
export type DeliveryAction = "integrate" | "push" | "pull_request";
export type Delivery = {
  id: number; chat_id: number; rev: number; state: "preview" | "running" | "done" | "refused" | "inspection" | "acknowledged";
  result: string | null;
  snapshot: {
    target: DraftTarget; action: DeliveryAction; commit_sha: string;
    workspace_path: string; checkout_head: string; checkout_branch: string;
    remote_url: string | null; delivery_branch: string; github_repo: string | null;
    base_branch: string | null; base_sha: string | null;
  };
};
export type Changes = {
  workspace_path: string; workspace_epoch: number; head: string | null; branch: string | null; issues: string[];
  staged: FileDiff[]; unstaged: FileDiff[]; untracked: string[];
  drafts: Draft[]; deliveries: Delivery[];
};
export type DraftReview = { draft: Draft; files: FileDiff[]; findings: RunEvent[] };
export type CommitEntry = { path: string; kind: string; size: number | null };
export type CommitFile = { path: string; commit_sha: string; text: string | null; reason: string | null };
export type RetainedInspection = { chat_id: number; target: DraftTarget; path: string; branch: string; head: string; staged: FileDiff[]; unstaged: FileDiff[]; untracked: string[] };
export const committedTree = (chat: number, target: DraftTarget): Promise<CommitEntry[]> => post(`/chats/${chat}/draft/tree`, target);
export const committedFile = (chat: number, target: DraftTarget, path: string): Promise<CommitFile> => post(`/chats/${chat}/draft/file`, { target, path });
export const inspectRetained = (chat: number, target: DraftTarget): Promise<RetainedInspection> => post(`/chats/${chat}/retained/inspect`, target);
export type RetainedFile = { path: string; text: string | null; reason: string | null };
export const retainedFile = (chat: number, target: DraftTarget, path: string): Promise<RetainedFile> => post(`/chats/${chat}/retained/file`, { target, path });
export const keepRetained = (chat: number, target: DraftTarget, reason: string): Promise<unknown> => post(`/chats/${chat}/retained/keep`, { target, reason });
export const chatChanges = (chat: number): Promise<Changes> => api(`/chats/${chat}/changes`);
export const reviewDraft = (chat: number, target: DraftTarget): Promise<DraftReview> => post(`/chats/${chat}/draft/review`, target);
export const recordFinding = (chat: number, target: DraftTarget, body: string): Promise<unknown> => post(`/chats/${chat}/draft/findings`, { target, body });
export const previewDelivery = (chat: number, target: DraftTarget, action: DeliveryAction): Promise<Delivery> => post(`/chats/${chat}/delivery/preview`, { target, action });
export type DeliveryInspection = { delivery: Delivery; checkout: { head: string; branch: string | null; status: string } | null };
export const inspectDelivery = (chat: number, delivery: Delivery): Promise<DeliveryInspection> => post(`/chats/${chat}/delivery/inspect`, { delivery_id: delivery.id, expect_revision: delivery.rev });
export const acknowledgeDelivery = (chat: number, inspection: DeliveryInspection, reason: string): Promise<DeliveryInspection> => post(`/chats/${chat}/delivery/acknowledge`, { delivery_id: inspection.delivery.id, expect_revision: inspection.delivery.rev, checkout: inspection.checkout, reason });
export const approveDelivery = (chat: number, delivery: Delivery): Promise<Delivery> => post(`/chats/${chat}/delivery/approve`, { delivery_id: delivery.id, expect_revision: delivery.rev });
