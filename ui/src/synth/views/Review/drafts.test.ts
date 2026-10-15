import { expect, it } from "vitest";
import { captureReview, currentReview, openReviewView, reviewDraftState, type ReviewViews } from "./drafts";
import type { Changes, DraftReview } from "../../../changes-api";
const review = (): DraftReview => ({ draft: { target: { run_id: 1, slice_key: "S1", revision: 2 }, base_sha: "base", commit_sha: "commit", branch: "draft", lease_state: "released", worktree_path: null }, files: [], findings: [] });
const changes = (): Changes => ({ workspace_path: "/fixture", workspace_epoch: 3, head: "head", branch: "main", issues: [], staged: [], unstaged: [], untracked: [], drafts: [review().draft], deliveries: [] });
const scope = { workspace: "/fixture", epoch: 3 };
it("captures semantic identities and refuses missing generation authority", () => {
  const first = captureReview(review(), scope), reordered = review();
  reordered.draft.target = { revision: 2, slice_key: "S1", run_id: 1 };
  expect(captureReview(reordered, scope).key).toBe(first.key);
  expect(captureReview(review(), { ...scope, epoch: 4 }).key).not.toBe(first.key);
  expect(() => captureReview(review(), { ...scope, epoch: NaN })).toThrow(/generation/);
});
it("keeps currentness exact across checkout generations and draft identities", () => {
  const captured = captureReview(review(), scope);
  expect(currentReview(captured, changes())).toBe(true);
  expect(currentReview(captured, { ...changes(), workspace_epoch: 4 })).toBe(false);
  for (const patch of [{ base_sha: "other" }, { commit_sha: "other" }, { branch: "other" }, { target: { ...review().draft.target, revision: 3 } }]) {
    expect(currentReview(captured, { ...changes(), drafts: [{ ...review().draft, ...patch }] })).toBe(false);
  }
});
it("retains dirty or not-yet-reported owners but prunes clean inactive reviews", () => {
  const first = captureReview(review(), scope), second = captureReview(review(), { ...scope, epoch: 4 });
  let views: ReviewViews = openReviewView({ active: null, sessions: [] }, first);
  views = openReviewView(views, second);
  expect(views.sessions).toHaveLength(2);
  views = reviewDraftState(views, first.key, true);
  views = openReviewView(views, first);
  views = reviewDraftState(views, second.key, false);
  expect(views.sessions.map(session => session.key)).toEqual([first.key]);
  expect(reviewDraftState(views, first.key, true)).toBe(views);
  views = reviewDraftState(views, first.key, false);
  expect(openReviewView(views, second).sessions.map(session => session.key)).toEqual([second.key]);
});
it("never substitutes newly fetched files into an already captured exact review", () => {
  const first = captureReview(review(), scope), changed = review();
  changed.files = [{ path: "unexpected.txt", old_path: null, status: "added", binary: false, additions: 0, deletions: 0, hunks: [] }];
  const views = openReviewView(openReviewView({ active: null, sessions: [] }, first), captureReview(changed, scope));
  expect(views.sessions).toHaveLength(1);
  expect(views.sessions[0]?.review.files).toBe(first.review.files);
});
