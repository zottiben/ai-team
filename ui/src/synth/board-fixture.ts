import { PLAN_STATUSES, type ChatPlan } from "../plan-api";

/** Synthetic, in-memory data for the real retained planning controller. */
export function fixtureBoard(emptyActive = false) {
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
  if (emptyActive && snapshot.bundle) snapshot.bundle.slices = snapshot.bundle.slices.map(slice => slice.status === "active" ? { ...slice, status: "ready" } : slice);
  let failure: "refuse" | "lost" | null = null;
  const requests: unknown[] = [];
  return {
    read: () => snapshot,
    state: () => ({ snapshot: structuredClone(snapshot), requests: structuredClone(requests) }),
    failNext: (mode: "refuse" | "lost") => { failure = mode; },
    flags(flags: { archived?: boolean; frozen?: boolean }) { snapshot = { ...snapshot, ...flags }; },
    change(body: unknown) {
      requests.push(body);
      if (!body || typeof body !== "object" || !("action" in body) || body.action !== "set_slice_status"
        || !("key" in body) || typeof body.key !== "string" || !("status" in body) || !("expect_revision" in body)
        || !Object.keys(body).every(key => ["action", "key", "status", "expect_revision", "reason"].includes(key))) return { status: 400, body: { error: "Only an exact synthetic status change is allowed." } };
      const status = PLAN_STATUSES.find(status => status === body.status), slice = snapshot.bundle?.slices.find(slice => slice.key === body.key);
      const reason = "reason" in body && typeof body.reason === "string" ? body.reason : null;
      if (!status || !slice || body.expect_revision !== snapshot.revision || snapshot.frozen || snapshot.archived
        || (status === "blocked" && (!reason?.trim() || reason.length > 16000))) return { status: 409, body: { error: "Synthetic status scope or blocking reason is stale or unavailable." } };
      const fail = failure; failure = null;
      if (fail === "refuse") return { status: 409, body: { error: "Synthetic status refused before writing." } };
      snapshot = structuredClone(snapshot); snapshot.revision++;
      const changed = snapshot.bundle!.slices.find(candidate => candidate.key === slice.key)!;
      changed.status = status; changed.blocked_reason = status === "blocked" ? reason : null; changed.rev++;
      return fail === "lost" ? { status: 500, body: { error: "Synthetic response lost after the status was stored." } } : { status: 200, body: snapshot };
    },
    advance() {
      snapshot = structuredClone(snapshot); snapshot.revision++;
      snapshot.bundle!.sections[0] = { key: "scope", title: "Refreshed scope", body: "New synthetic scope", rev: 2 };
      snapshot.bundle!.questions[0] = { ...snapshot.bundle!.questions[0]!, status: "answered", answer: "A synthetic peer answer" };
    },
  };
}
