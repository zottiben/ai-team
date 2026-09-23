import { expect, it } from "vitest";

import type { Worktree } from "./api";
import { nested, workspaceDetail, workspaceName, workspaceTitle } from "./tree";

function tree(path: string, over: Partial<Worktree> = {}): Worktree {
  return {
    name: path.split("/").pop() ?? path,
    path,
    status: "leased",
    lease_holder: null,
    processes: [],
    branch: null,
    main: false,
    kind: "manual",
    parent: "/repo",
    ...over,
  };
}

it("draws every checkout straight after the one it sits under, a stack deepest last", () => {
  const trees = [
    tree("/repo", { main: true, kind: "main", parent: null, branch: "ai-team/run-5" }),
    tree("/repo/.claude/worktrees/side", { branch: "side" }),
    tree("/awt/1", { kind: "pr", slice_key: "PR1", plan: "p", parent: "/repo" }),
    tree("/awt/2", { kind: "pr", slice_key: "PR2", plan: "p", parent: "/awt/1" }),
    tree("/awt/3", { kind: "pr", slice_key: "PR1", plan: "q", parent: "/repo/.claude/worktrees/side" }),
  ];

  expect(nested(trees).map(({ tree, depth }) => `${depth} ${workspaceName(tree)}`)).toEqual([
    "0 main",
    "1 side",
    "2 PR1",
    "1 PR1",
    "2 PR2",
  ]);
});

it("keeps a checkout whose parent has gone, at the top", () => {
  const trees = [tree("/awt/2", { kind: "pr", slice_key: "PR2", parent: "/awt/1" })];
  expect(nested(trees).map(({ depth }) => depth)).toEqual([0]);
});

it("names main and says what it is on, and a PR by its key and plan", () => {
  const main = tree("/repo", { main: true, kind: "main", parent: null, branch: "ai-team/run-5" });
  expect([workspaceName(main), workspaceDetail(main)]).toEqual(["main", "ai-team/run-5"]);
  const pr = tree("/awt/1", { kind: "pr", slice_key: "PR1", plan: "csv-export" });
  expect([workspaceName(pr), workspaceDetail(pr)]).toEqual(["PR1", "csv-export"]);
  const side = tree("/side", { branch: "fix/thing" });
  expect([workspaceName(side), workspaceDetail(side)]).toEqual(["fix/thing", null]);
});

it("heads a page with what the checkout is", () => {
  expect(workspaceTitle(tree("/repo", { main: true, kind: "main", parent: null }))).toBe(
    "main checkout",
  );
  expect(
    workspaceTitle(tree("/awt/2", { kind: "pr", slice_key: "PR2", branch: "p/pr2" })),
  ).toBe("PR2 · p/pr2");
  expect(workspaceTitle(tree("/side", { branch: "fix/thing" }))).toBe("fix/thing");
});
