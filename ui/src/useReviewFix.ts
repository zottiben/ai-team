import { useRef } from "react";
import { submitReviewFix, type ReviewFix, type ReviewRequest } from "./review-api";

// Keep the exact request after an uncertain response, even if polling changes the diff.
// Editing the comment is a new request; a retry never silently follows a new checkout.
export function useReviewFix(chat: number) {
  const pending = useRef(new Map<string, { body: string; input: ReviewRequest }>());
  return async (review: ReviewFix, workspace_epoch: number) => {
    const key = JSON.stringify([chat, review.kind, review.kind === "checkout"
      ? [review.finding.area, review.finding.path, review.finding.side, review.finding.line]
      : [review.target.run_id, review.target.slice_key, review.anchor]]);
    const body = review.kind === "checkout" ? review.finding.body : review.body;
    const previous = pending.current.get(key);
    const request = previous?.body === body ? previous : { body, input: { request_id: crypto.randomUUID(), workspace_epoch, review } };
    pending.current.set(key, request);
    await submitReviewFix(chat, request.input);
    pending.current.delete(key);
  };
}
