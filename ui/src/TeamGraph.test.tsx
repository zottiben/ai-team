import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { TeamGraph, WorkGraph } from "./TeamGraph";
import type { Board, Member } from "./api";

function member(change: Partial<Member>): Member {
  return {
    agent_id: 1,
    role: "backend",
    name: "Backend",
    provider: "claude",
    model: "claude-opus-5",
    read_only: false,
    zone: "crates/**",
    doing: "idle",
    node_run_id: 12,
    run_id: 7,
    slice_key: null,
    branch: null,
    attempt: 1,
    blocked_reason: null,
    last_said: null,
    reachable: false,
    session_active: true,
    context_tokens: 640_000,
    turns: 1,
    tokens_in: 10,
    tokens_out: 2,
    ...change,
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("makes a long-running command the primary card content without repeating ownership", () => {
  const started = new Date(Date.now() - 125_000).toISOString();
  render(
    <TeamGraph
      members={[
        member({
          doing: "working",
          slice_key: "S1",
          activity: {
            kind: "tool_call",
            summary: "bash · npm test",
            at: started,
          },
        }),
      ]}
      workspace="/repo/task"
    />,
  );

  expect(screen.getByText("bash · npm test")).toBeDefined();
  expect(screen.getByText(/running for 2m/)).toBeDefined();
  expect(screen.getByRole("progressbar", { name: "Backend current command" })).toBeDefined();
  expect(screen.queryByText(/owns crates/)).toBeNull();
});

it("keeps the age of recent activity moving after work settles", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-23T12:00:00Z"));
  render(
    <TeamGraph
      members={[
        member({
          doing: "idle",
          last_said: "checks passed",
          activity: {
            kind: "done",
            summary: "turn settled",
            at: "2026-09-23T11:59:59Z",
          },
        }),
      ]}
      workspace="/repo/task"
    />,
  );

  expect(screen.getByText("1s ago")).toBeDefined();
  act(() => vi.advanceTimersByTime(60_000));
  expect(screen.getByText("1m 1s ago")).toBeDefined();
});

it("draws the role flow, exact context occupancy, and fresh-session action", async () => {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = String(input).replace(/^\/api/, "");
      calls.push(path);
      if (path === "/models") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            models: [
              {
                provider: "claude",
                runtime_provider: "claude-subscription",
                model: "claude-opus-5",
                context: "1M",
                context_tokens: 1_000_000,
                max_output: "128K",
                thinking: true,
                images: true,
              },
            ],
            error: null,
          }),
        });
      }
      if (path === "/runs/7/nodes/12/reset-session") {
        return Promise.resolve({
          ok: true,
          json: async () => ({ resetting: true, run_id: 7, node_run_id: 12 }),
        });
      }
      return Promise.resolve({ ok: false, status: 404, json: async () => ({ error: path }) });
    }),
  );

  render(
    <TeamGraph
      members={[
        member({ agent_id: 2, role: "orchestrator", name: "Orchestrator", session_active: false }),
        member({ agent_id: 3, role: "planner", name: "Planner", session_active: false }),
        member({}),
        member({ agent_id: 4, role: "verifier", name: "Verifier", session_active: false }),
      ]}
      workspace="/repo/task"
    />,
  );

  expect(screen.getByRole("region", { name: "coordinate" })).toBeDefined();
  expect(screen.getByRole("region", { name: "plan" })).toBeDefined();
  expect(screen.getByRole("region", { name: "make" })).toBeDefined();
  expect(screen.getByRole("region", { name: "check" })).toBeDefined();
  expect((await screen.findAllByText("640K / 1.0M")).length).toBe(4);

  await userEvent.click(screen.getByRole("button", { name: "New session" }));
  await waitFor(() =>
    expect(calls).toContain("/runs/7/nodes/12/reset-session"),
  );
});

it("draws work from board evidence rather than treating every configured seat as work", () => {
  const board: Board = {
    plan: { plan: "widget", title: "Widget plan", status: "active", slice: "S1" },
    next_step: "Build S1",
    slices: [
      {
        id: 1,
        plan_id: 1,
        key: "S1",
        title: "Build the API",
        status: "active",
        ord: 1,
        scope_md: null,
        demo_md: null,
        estimate_files: 2,
        branch: null,
        base_branch: null,
        pr_url: null,
        worktree_path: null,
        claimed_by: "worker",
        claimed_at: null,
        blocked_reason: null,
        started_at: null,
        completed_at: null,
        rev: 1,
        updated_at: null,
        owner: "backend",
        touches: ["src/**"],
      },
      {
        id: 2,
        plan_id: 1,
        key: "S2",
        title: "Build the view",
        status: "in_review",
        ord: 2,
        scope_md: null,
        demo_md: null,
        estimate_files: 2,
        branch: "ai-team/S2",
        base_branch: null,
        pr_url: null,
        worktree_path: null,
        claimed_by: null,
        claimed_at: null,
        blocked_reason: null,
        started_at: null,
        completed_at: null,
        rev: 1,
        updated_at: null,
        owner: "frontend",
        crew: ["frontend", "backend"],
        touches: ["ui/**"],
      },
    ],
  };

  const { container } = render(
    <WorkGraph
      board={board}
      members={[
        member({ role: "orchestrator", name: "Orchestrator", doing: "idle" }),
        member({ agent_id: 2, role: "planner", name: "Planner", doing: "idle" }),
        member({ agent_id: 3, role: "verifier", name: "Verifier", doing: "working", slice_key: "S1" }),
        member({ agent_id: 4, role: "reviewer", name: "Reviewer", doing: "untouched" }),
      ]}
    />,
  );

  expect(screen.getByRole("region", { name: "plan work" }).textContent).toContain("2/2 slices shaped");
  expect(screen.getByRole("region", { name: "make work" }).textContent).toContain("2/2 slices delivered");
  expect(screen.getByRole("region", { name: "check work" }).textContent).toContain("1/2 verified");
  expect(screen.getByText("Build the API")).toBeDefined();
  expect(screen.getByText("built by backend")).toBeDefined();
  // A pull request built as tasks is built by its whole crew, in the order they build.
  expect(screen.getByText("built by frontend, then backend")).toBeDefined();
  expect(container.querySelector('[aria-label="S1 route"] [data-state="live"]')?.textContent).toBe("check");
  expect(screen.queryByText("Reviewer")).toBeNull();
  expect(screen.queryByText(/tokens/)).toBeNull();
});

it("offers a direct build action when the current plan already has ready work", async () => {
  const build = vi.fn();
  const board: Board = {
    plan: { plan: "legacy", title: "Ready plan", status: "active", slice: null },
    next_step: "Build S1",
    slices: [{
      id: 1,
      plan_id: 1,
      key: "S1",
      title: "Ready work",
      status: "ready",
      ord: 1,
      scope_md: null,
      demo_md: null,
      estimate_files: null,
      branch: null,
      base_branch: null,
      pr_url: null,
      worktree_path: null,
      claimed_by: null,
      claimed_at: null,
      blocked_reason: null,
      started_at: null,
      completed_at: null,
      rev: 1,
      updated_at: null,
      owner: "backend",
      touches: ["src/**"],
    }],
  };

  render(<WorkGraph board={board} members={[member({})]} onBuildReady={build} />);
  await userEvent.click(screen.getByRole("button", { name: "Build 1 ready slice" }));
  expect(build).toHaveBeenCalledWith(false);
});

it("in a pull request's worktree offers its builders, not the seats that coordinate", () => {
  const board: Board = {
    plan: { plan: "csv", title: "CSV", status: "active", slice: null },
    next_step: null,
    slices: [],
  };
  const crew = [
    member({ role: "orchestrator", name: "Orchestrator", doing: "idle" }),
    member({ agent_id: 2, role: "planner", name: "Planner", doing: "idle" }),
    member({ agent_id: 3, role: "frontend", name: "Frontend", doing: "idle" }),
  ];

  render(<WorkGraph board={board} members={crew} onTalk={() => {}} coordinates={false} />);

  expect(screen.getByRole("button", { name: "Talk to Frontend" })).toBeDefined();
  expect(screen.queryByRole("button", { name: "Talk to Orchestrator" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Talk to Planner" })).toBeNull();
});

it("makes an approval-held board an explicit approve-and-build action", async () => {
  const build = vi.fn();
  const board: Board = {
    plan: { plan: "legacy", title: "Held plan", status: "active", slice: null },
    next_step: null,
    slices: [{
      id: 1,
      plan_id: 1,
      key: "S1",
      title: "Held work",
      status: "blocked",
      ord: 1,
      scope_md: null,
      demo_md: null,
      estimate_files: null,
      branch: null,
      base_branch: null,
      pr_url: null,
      worktree_path: null,
      claimed_by: null,
      claimed_at: null,
      blocked_reason: "Awaiting plan approval from ai-team",
      started_at: null,
      completed_at: null,
      rev: 1,
      updated_at: null,
      owner: "backend",
      touches: ["src/**"],
      approval_held: true,
    }],
  };

  render(<WorkGraph board={board} members={[member({})]} onBuildReady={build} />);
  expect(screen.getByText("awaiting approval")).toBeDefined();
  expect(screen.getByText("Awaiting plan approval from ai-team").className).not.toContain("error");
  await userEvent.click(screen.getByRole("button", { name: "Approve plan & build" }));
  expect(build).toHaveBeenCalledWith(true);
});

it("offers the next ask-gated delivery boundary on a verified local branch", async () => {
  const deliver = vi.fn();
  const maker = member({ slice_key: "S1", branch: "ai-team/s1" });
  const delivery: NonNullable<Board["slices"][number]["delivery"]> = {
    run_id: 7,
    node_run_id: 12,
    branch: "ai-team/s1",
    pushed_at: null,
    pr_url: null,
    merge_requested_at: null,
    delivery_claim: null,
    delivery_claimed_at: null,
    delivery_error: null,
    policy: { push: "ask" as const, pr: "ask" as const, merge: "ask" as const },
    remote: null,
  };
  const board: Board = {
    plan: { plan: "widget", title: "Widget", status: "active", slice: null },
    next_step: null,
    slices: [{
      id: 1,
      plan_id: 1,
      key: "S1",
      title: "Built work",
      status: "in_review",
      ord: 1,
      scope_md: null,
      demo_md: null,
      estimate_files: 1,
      branch: "ai-team/s1",
      base_branch: "main",
      pr_url: null,
      worktree_path: null,
      claimed_by: null,
      claimed_at: null,
      blocked_reason: null,
      started_at: null,
      completed_at: null,
      rev: 1,
      updated_at: null,
      owner: "backend",
      touches: ["src/**"],
      delivery,
    }],
  };

  const view = render(<WorkGraph board={board} members={[maker]} onDeliver={deliver} />);
  expect(screen.getByText("committed")).toBeDefined();
  await userEvent.click(screen.getByRole("button", { name: "Push branch" }));
  expect(deliver).toHaveBeenCalledWith(delivery, "push");

  const withChecks: Board = {
    ...board,
    slices: [{
      ...board.slices[0]!,
      delivery: {
        ...delivery,
        pushed_at: "2026-09-22T12:00:00Z",
        pr_url: "https://github.com/acme/widget/pull/7",
        remote: { pr_state: "open", checks: "pending" },
      },
    }],
  };
  view.rerender(<WorkGraph board={withChecks} members={[maker]} onDeliver={deliver} />);
  expect(screen.getByText("checks pending")).toBeDefined();
  expect(screen.getByRole("button", { name: "Merge after checks" })).toBeDefined();
});

it("labels the orchestrator reset as a required handoff", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: true, json: async () => ({ models: [], error: null }) })),
  );
  render(
    <TeamGraph
      members={[member({ role: "orchestrator", name: "Orchestrator" })]}
      workspace="/repo/task"
    />,
  );
  expect(screen.getByRole("button", { name: "Handoff & reset" })).toBeDefined();
});
