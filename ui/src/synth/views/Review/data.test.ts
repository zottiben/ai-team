import { expect, it } from "vitest";
import { fixtureReview } from "../../review-fixture";
import { barHeight, hunkBars, latestOperation, pushedCommit, receiptScope } from "./data";
import type { CheckoutOperation } from "../../../checkout-api";

it("uses four positional bins of real lines without mutating or inventing a hunk", () => {
  const { checkout } = fixtureReview(8), file = checkout.unstaged[0]!;
  const before = JSON.stringify(file), result = hunkBars(file);
  expect(result[0]?.bars).toEqual(Array.from({ length: 4 }, () => ({ added: 2, removed: 0 })));
  expect(result[0]?.added).toBe(8); expect(result[0]?.removed).toBe(0);
  expect(barHeight(0)).toBe(0); expect(barHeight(100)).toBe(24); expect(barHeight(1)).toBe(.9);
  expect(JSON.stringify(file)).toBe(before);
  expect(hunkBars({ ...file, hunks: [] })).toEqual([]);
});
it("never replaces a newer refusal/unknown with an older successful push or a PR", () => {
  const { checkout } = fixtureReview(0);
  const operation = (id: number, state: string): CheckoutOperation => ({ id, state, chat_id: 1, rev: 1, result: null,
    snapshot: { workspace: checkout.workspace, workspace_epoch: 3, head: checkout.head, branch: checkout.branch, fingerprint: checkout.fingerprint,
      action: { kind: "push_branch", branch: "fixture/work", commit: "a-different-draft-commit" }, remote: null } });
  const done = operation(1, "done"), refused = operation(2, "refused"), preview = operation(4, "preview");
  const pr = { ...operation(3, "done"), snapshot: { ...done.snapshot, action: { kind: "pull_request" as const } } };
  expect(latestOperation([done, preview, pr, refused], ["push", "push_branch"])).toBe(refused);
  expect(pushedCommit(done)).toBe("a-different-draft-commit");
  expect(pushedCommit(pr)).toBeNull();
  expect(receiptScope(done, checkout)).toBe("current");
  expect(receiptScope(done, { ...checkout, workspace_epoch: 4 })).toBe("prior");
  delete done.snapshot.workspace_epoch;
  expect(receiptScope(done, checkout)).toBe("unknown");
});
