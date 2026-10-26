import type { Draft, DraftReview } from "../changes-api";

/** Synthetic committed reviews only. No repository, repair or delivery action exists. */
export function fixtureTeamReviews() {
  let drafts: Draft[] = ["S1", "S2"].map((slice_key, index) => ({ target: { run_id: 14, slice_key, revision: 1 },
    base_sha: "a".repeat(40), commit_sha: (index ? "c" : "b").repeat(40), branch: `fixture/${slice_key}`, lease_state: "released", worktree_path: null }));
  return {
    drafts: () => drafts,
    replace() { drafts = drafts.map((draft, index) => index ? draft : { ...draft, target: { ...draft.target, revision: 2 }, commit_sha: "d".repeat(40) }); return drafts; },
    review(target: unknown): DraftReview | null {
      if (!target || typeof target !== "object" || !("run_id" in target) || !("slice_key" in target) || !("revision" in target)) return null;
      const draft = drafts.find(draft => draft.target.run_id === target.run_id && draft.target.slice_key === target.slice_key && draft.target.revision === target.revision);
      if (!draft) return null;
      return { draft, findings: [], files: [{ path: `src/${draft.target.slice_key}.ts`, old_path: null, status: "added", binary: false, additions: 1, deletions: 0,
        hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text: `export const revision = ${draft.target.revision}; // synthetic ${draft.target.slice_key}` }] }] }] };
    },
  };
}
