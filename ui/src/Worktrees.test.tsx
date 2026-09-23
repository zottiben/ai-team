import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { Worktrees } from "./Worktrees";
import type { Worktree } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function tree(over: Partial<Worktree> = {}): Worktree {
  return {
    name: "1",
    path: "/Users/me/.awt/nodifi-data-178f5b/1/nodifi-data",
    status: "leased",
    lease_holder: null,
    processes: [],
    branch: "chore/review-7223",
    main: false,
    kind: "manual",
    parent: "/Users/me/nodifi-data",
    ...over,
  };
}

function stub(pool: Worktree[] | { error: string }) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Array.isArray(pool)
        ? Promise.resolve({ ok: true, json: async () => pool })
        : Promise.resolve({ ok: false, status: 400, json: async () => pool }),
    ),
  );
}

it("leads with the branch, because the directory names do not distinguish anything", async () => {
  // `awt` calls them 1, 2, 3 and 4. Without the branch this is four identical rows.
  stub([tree(), tree({ name: "2", branch: "feature/app-lifecycle-capture" })]);
  render(<Worktrees project="nodifi-data" tick={0} />);

  expect(await screen.findByText("chore/review-7223")).toBeDefined();
  expect(screen.getByText("feature/app-lifecycle-capture")).toBeDefined();
});

it("calls out an orphaned lease, which is the state that needs a person", async () => {
  // `awt` will not hand the tree out and nobody is holding it, so it sits there costing
  // a slot until somebody returns it - and says so with the command that does it.
  stub([
    tree({
      lease_holder:
        "orphaned: machine restarted while in use; resume with 'awt enter' or release with 'awt return'",
    }),
  ]);
  render(<Worktrees project="nodifi-data" tick={0} />);

  expect(await screen.findByText("orphaned")).toBeDefined();
  expect(screen.getByText(/awt return /)).toBeDefined();
});

it("a held lease leads with its run slice and role while retaining the branch", async () => {
  // The pool number is awt identity. The holder is the human-meaningful answer to what
  // this checkout is doing, while the branch remains useful review detail.
  stub([tree({ lease_holder: "ai-team run-7 S1 frontend", branch: "ai-team/s1" })]);
  render(<Worktrees project="nodifi-data" tick={0} />);

  expect(await screen.findByText("ai-team run-7 S1 frontend")).toBeDefined();
  expect(screen.getByText("ai-team/s1")).toBeDefined();
  expect(screen.getByText("leased")).toBeDefined();
  expect(screen.queryByText("orphaned")).toBeNull();
});

it("says what is running in a worktree, without repeating a name per process", async () => {
  stub([
    tree({
      processes: [
        { pid: 1, name: "nvim" },
        { pid: 2, name: "nvim" },
        { pid: 3, name: "zsh" },
      ],
    }),
  ]);
  render(<Worktrees project="nodifi-data" tick={0} />);

  expect(await screen.findByText(/3 processes: nvim, zsh/)).toBeDefined();
});

it("a detached worktree says so rather than showing a blank", async () => {
  stub([tree({ branch: null })]);
  render(<Worktrees project="nodifi-data" tick={0} />);
  expect(await screen.findByText("detached")).toBeDefined();
});

it("an empty pool explains how one is made, rather than showing nothing", async () => {
  stub([]);
  render(<Worktrees project="nodifi-data" tick={0} />);
  expect(await screen.findByText(/No worktrees yet/)).toBeDefined();
});

it("a project with no checkout says so rather than reading as no worktrees", async () => {
  stub({ error: "that project has no checkout, so it has no worktrees" });
  render(<Worktrees project="nodifi-data" tick={0} />);
  expect(await screen.findByText(/no checkout/)).toBeDefined();
});
