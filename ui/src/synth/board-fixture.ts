import { PLAN_STATUSES, type ChatPlan } from "../plan-api";

/** Synthetic, in-memory data for the real retained planning controller. */
export function fixtureBoard() {
  let snapshot: ChatPlan = {
    chat_id: 1, project_id: 1, revision: 4, archived: false, frozen: false,
    bundle: {
      plan: { id: 1, slug: "fixture-plan", title: "Synthetic retained planning", summary: "Isolated fixture; no provider, Git, persisted plan or execution.", status: "ready" },
      sections: [{ key: "scope", title: "Scope", body: "Original synthetic scope", rev: 1 }, { key: "design", title: "Design", body: "Original synthetic design", rev: 1 }],
      slices: Array.from({ length: 100 }, (_, index) => ({
        id: index + 1, key: `S${index + 1}`, title: `Synthetic step ${index + 1}`, status: PLAN_STATUSES[(index + 1) % PLAN_STATUSES.length]!,
        scope_md: `Synthetic scope ${index + 1}\n\nTouches: src/**`, demo_md: "Inspect the isolated fixture. No commands run.",
        blocked_reason: (index + 1) % PLAN_STATUSES.length === 4 ? "Synthetic decision needed" : null,
        branch: `fixture/S${index + 1}`, estimate_files: index % 4, claimed_by: null, worktree_path: null, pr_url: null, rev: 1,
      })),
      questions: [{ id: 1, body: "Which synthetic behavior should we retain?", status: "open", answer: null, slice_key: "S1" }],
      decisions: [], gotchas: [], log: [],
    },
  };
  return {
    read: () => snapshot,
    advance() {
      snapshot = structuredClone(snapshot); snapshot.revision++;
      snapshot.bundle!.sections[0] = { key: "scope", title: "Refreshed scope", body: "New synthetic scope", rev: 2 };
      snapshot.bundle!.questions[0] = { ...snapshot.bundle!.questions[0]!, status: "answered", answer: "A synthetic peer answer" };
    },
  };
}
