import type { FileDiff } from "../api";
import type { CheckoutAction, CheckoutOperation, CheckoutState } from "../checkout-api";
import type { Changes } from "../changes-api";

export function fixturePreview(state: CheckoutState, fingerprint: unknown, value: unknown, id: number): CheckoutOperation | null {
  if (fingerprint !== state.fingerprint || !value || typeof value !== "object" || !("kind" in value)) return null;
  let action: CheckoutAction;
  if ((value.kind === "stage" || value.kind === "unstage") && "path" in value && typeof value.path === "string") action = { kind: value.kind, path: value.path };
  else if (value.kind === "commit" && "message" in value && typeof value.message === "string" && value.message.trim() && state.staged.length) action = { kind: "commit", message: value.message };
  else return null;
  const selected = "path" in action ? action.path : null;
  const files = [...state.staged.map(file => ({ area: "staged", file, revision: state.staged_revisions?.[file.path] ?? state.fingerprint, reason: null as string | null })),
    ...(action.kind === "stage" ? [...state.unstaged.map(file => ({ area: "unstaged", file, revision: state.unstaged_revisions?.[file.path] ?? state.fingerprint, reason: null })),
      ...state.untracked_files.map(entry => ({ area: "untracked", file: entry.file, revision: entry.fingerprint, reason: entry.reason }))] : [])]
    .filter(entry => selected === null || entry.file.path === selected).map(entry => {
      const pinned = structuredClone(entry);
      const omitted = new TextEncoder().encode(JSON.stringify(pinned)).length > 500_000;
      if (omitted) { pinned.file.hunks = []; pinned.reason = "Synthetic preview omission: inspect the large fixture in the main diff. No backend action is available."; }
      return { ...pinned, hunks_omitted: omitted };
    });
  if (!files.length) return null;
  return { id, chat_id: 1, rev: 1, state: "preview", result: null,
    snapshot: { workspace: state.workspace, workspace_epoch: state.workspace_epoch, head: state.head, branch: state.branch, fingerprint: state.fingerprint, action, remote: null },
    evidence: { basis: "Synthetic captured fixture — no Git, process or backend request", files_total: files.length, omitted_files: 0, files, note: "This fixture never executes approvals." } };
}

export function fixtureReview(lines = 10_000): { checkout: CheckoutState; changes: Changes } {
  const files: FileDiff[] = Array.from({ length: 6 }, (_, n) => ({
    path: n === 0 ? "src/large.ts" : `src/feature-${n}/a-long-but-real-fixture-path/module-${n}.ts`, old_path: n === 2 ? "src/old-module.ts" : null,
    status: n === 2 ? "renamed" : "modified", binary: false,
    additions: n === 0 ? lines : 4, deletions: n === 0 ? 0 : 1,
    hunks: [{ header: n === 0 ? `@@ -0,0 +1,${lines} @@` : "@@ -1 +1,4 @@", old_start: n === 0 ? 0 : 1, new_start: 1,
      lines: n === 0 ? Array.from({ length: lines }, (_, index) => ({ kind: "added", old: null, new: index + 1, text: `export const sample${index + 1} = ${index + 1}; // synthetic line, never executed` }))
        : [{ kind: "removed", old: 1, new: null, text: "const previous = false;" }, ...Array.from({ length: 4 }, (_, index) => ({ kind: "added" as const, old: null, new: index + 1, text: `const next${index + 1} = true;` }))] }],
  }));
  const checkout: CheckoutState = { workspace: "/fixture/demo", workspace_epoch: 3, head: "abcdef0123456789abcdef0123456789abcdef0123", branch: "fixture/review", fingerprint: "fixture-diff-one",
    staged: [], unstaged: files, unstaged_revisions: Object.fromEntries(files.map(file => [file.path, `fixture-${file.path}-one`])),
    untracked: ["new.txt"], untracked_files: [{ file: { path: "new.txt", old_path: null, status: "added", binary: false, additions: 1, deletions: 0,
      hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text: "Synthetic untracked text" }] }] }, reason: null, fingerprint: "fixture-untracked-one" }], findings: [], operations: [] };
  return { checkout, changes: { workspace_path: checkout.workspace, workspace_epoch: checkout.workspace_epoch, head: checkout.head, branch: checkout.branch,
    issues: [], staged: [], unstaged: files, untracked: checkout.untracked, drafts: [], deliveries: [] } };
}
