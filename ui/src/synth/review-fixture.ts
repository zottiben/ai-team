import type { FileDiff } from "../api";
import type { CheckoutState } from "../checkout-api";
import type { Changes } from "../changes-api";

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
