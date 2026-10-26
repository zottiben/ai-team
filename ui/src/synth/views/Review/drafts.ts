import type { Changes, DeliveryAction, DraftReview } from "../../../changes-api";

export const deliveryActions: Record<DeliveryAction, string> = { integrate: "Integrate locally", push: "Push draft", pull_request: "Open draft PR" };
export type ReviewScope = { workspace: string; epoch: number };
export type ReviewSession = ReviewScope & { key: string; review: DraftReview; dirty?: boolean };
export type ReviewViews = { active: string | null; sessions: ReviewSession[] };
export function captureReview(review: DraftReview, scope: ReviewScope): ReviewSession {
  const { draft } = review;
  if (!Number.isSafeInteger(scope.epoch) || scope.epoch < 0) throw Error("Checkout generation is unavailable; refresh before reviewing a draft.");
  return { ...scope, review, key: JSON.stringify([scope.workspace, scope.epoch, draft.target.run_id, draft.target.slice_key, draft.target.revision, draft.base_sha, draft.commit_sha, draft.branch]) };
}
export function currentReview(session: ReviewSession, changes: Changes): boolean {
  const selected = session.review.draft;
  return session.workspace === changes.workspace_path && session.epoch === changes.workspace_epoch && changes.drafts.some(draft =>
    draft.target.run_id === selected.target.run_id && draft.target.slice_key === selected.target.slice_key && draft.target.revision === selected.target.revision
    && draft.commit_sha === selected.commit_sha && draft.base_sha === selected.base_sha && draft.branch === selected.branch);
}
export function openReviewView(views: ReviewViews, next: ReviewSession): ReviewViews {
  const existing = views.sessions.find(session => session.key === next.key);
  const sessions = views.sessions.filter(session => session.key === next.key || session.dirty !== false);
  // An exact review is immutable. Refresh only its append-only findings, not its files.
  return { active: next.key, sessions: existing ? sessions.map(session => session.key === next.key
    ? { ...session, review: { ...session.review, findings: next.review.findings } } : session) : [...sessions, next] };
}
export function reviewDraftState(views: ReviewViews, key: string, dirty: boolean): ReviewViews {
  const session = views.sessions.find(session => session.key === key);
  if (!session) return views;
  if (!dirty && views.active !== key) return { ...views, sessions: views.sessions.filter(session => session.key !== key) };
  if (session.dirty === dirty) return views;
  return { ...views, sessions: views.sessions.map(session => session.key === key ? { ...session, dirty } : session) };
}
