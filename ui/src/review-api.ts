import { post } from "./api";
import type { CheckoutFinding } from "./checkout-api";
import type { DraftTarget } from "./changes-api";
export type ReviewFix = { kind: "checkout"; finding: CheckoutFinding } | { kind: "draft"; target: DraftTarget; body: string; anchor: { path: string; side: string; line: number } | null };
export type ReviewRequest = { request_id: string; workspace_epoch: number; review: ReviewFix };
export const submitReviewFix = (chat: number, input: ReviewRequest): Promise<unknown> => post(`/chats/${chat}/review-fix`, input);
