import type { PlanStatus } from "../../../plan-api";
import type { Signal } from "../../primitives/signal";

export const statusSignals: Record<PlanStatus, Signal> = {
  draft: "slice-draft", ready: "slice-ready", active: "slice-active", in_review: "slice-review",
  blocked: "slice-blocked", done: "slice-done", deferred: "slice-deferred",
};
