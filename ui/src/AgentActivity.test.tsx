import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { AgentActivity } from "./AgentActivity";
import type { Run, RunDetail } from "./api";

const RUN: Run = {
  id: 7,
  project_id: 1,
  prompt: "continue from the plan",
  status: "running",
  trigger: "manual",
  workspace_path: "/tmp/widget-task",
  created_at: "",
  started_at: "",
  ended_at: "",
};

function stub(
  run: Partial<Omit<RunDetail, "nodes" | "usage">> = {},
  recoverable = false,
  events?: unknown[],
) {
  const calls: Array<{ path: string; body?: unknown }> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const path = String(input).replace(/^\/api/, "");
      calls.push({ path, body: init?.body === undefined ? undefined : JSON.parse(String(init.body)) });
      if (path.startsWith("/runs/7/events")) {
        return Promise.resolve({
          ok: true,
          json: async () => events ?? [
            {
              id: 1,
              node_run_id: 11,
              kind: "cost",
              actor: "orchestrator",
              summary: "step finished",
              message: null,
              thinking: ["Checking the plan and the current branch"],
              at: "",
            },
            {
              id: 2,
              node_run_id: 11,
              kind: "note",
              actor: "orchestrator",
              summary: "I need an answer…",
              message: "I need an answer before I can continue.",
              thinking: [],
              at: "",
            },
            {
              id: 3,
              node_run_id: 11,
              kind: "cost",
              actor: "orchestrator",
              summary: "step finished",
              message: null,
              thinking: ["Checking the plan and the current branch"],
              at: "",
            },
            {
              id: 4,
              node_run_id: 12,
              kind: "note",
              actor: "frontend",
              summary: "I am checking the visual state",
              message: "I am checking the visual state.",
              thinking: [],
              at: "",
            },
          ],
        });
      }
      if (path.startsWith("/runs/7?")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            ...RUN,
            ...run,
            nodes: [
              {
                id: 11,
                role: "orchestrator",
                provider: "openai",
                model: "gpt",
                status: "running",
                attempt: 1,
                slice_key: null,
                worktree_path: "/tmp/widget",
                branch: "task",
                blocked_reason: null,
                session_id: "session-11",
                recoverable,
              },
              {
                id: 12,
                role: "frontend",
                provider: "openai",
                model: "gpt",
                status: "running",
                attempt: 2,
                slice_key: "W2",
                task_key: "T2",
                worktree_path: "/tmp/widget",
                branch: "task",
                blocked_reason: null,
                session_id: "session-12",
              },
            ],
            usage: { tokens_in: 1, tokens_out: 2, cache_read: 0, cache_write: 0 },
          }),
        });
      }
      if (path === "/runs/7/nodes/11/reply" || path === "/runs/7/nodes/12/reply") {
        return Promise.resolve({ ok: true, json: async () => ({ reached: "queued", waiting: 1 }) });
      }
      if (path === "/runs/7/nodes/11/resume") {
        return Promise.resolve({
          ok: true,
          json: async () => ({ resumed: true, run_id: 7, node_id: 11 }),
        });
      }
      if (path === "/runs/7/approve-plan") {
        return Promise.resolve({ ok: true, json: async () => ({ continued: true, run_id: 7 }) });
      }
      return Promise.resolve({ ok: false, status: 404, json: async () => ({ error: path }) });
    }),
  );
  return calls;
}

afterEach(() => vi.unstubAllGlobals());

it("says what empty means in a pull request's worktree, and nothing under an error", async () => {
  const { unmount } = render(
    <AgentActivity runs={[]} workspace="/awt/widget/2/widget" leaf tick={0} />,
  );
  expect(screen.getByText(/A run started above it dispatches its crew here/)).toBeDefined();
  expect(screen.queryByText(/Start a run/)).toBeNull();
  unmount();

  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve({
        ok: false,
        status: 400,
        json: async () => ({ error: "that run does not belong to the selected workspace" }),
      }),
    ),
  );
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);
  expect(await screen.findByText("that run does not belong to the selected workspace")).toBeDefined();
  expect(screen.queryByText(/Start a run/)).toBeNull();
});

it("shows thinking and the full answer instead of only the clipped event summary", async () => {
  stub();
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  expect(await screen.findByText("Checking the plan and the current branch")).toBeDefined();
  expect(screen.getAllByText("Checking the plan and the current branch")).toHaveLength(1);
  expect(await screen.findByText("I need an answer before I can continue.")).toBeDefined();
  expect(screen.getByText("continue from the plan")).toBeDefined();
});

it("reads an agent's markdown as the formatting it is, and what you typed as you typed it", async () => {
  stub({}, false, [
    {
      id: 1,
      node_run_id: 11,
      kind: "cost",
      actor: "orchestrator",
      summary: "step finished",
      message: null,
      thinking: ["**Checking baseline branch**"],
      at: "",
    },
    {
      id: 2,
      node_run_id: 11,
      kind: "note",
      actor: "orchestrator",
      summary: "Plan: shout",
      message:
        "## Grounding\n- `src/greet.sh` prints the greeting.\n\n**PR2 - Show both forms**\nOwner: frontend.",
      thinking: [],
      at: "",
    },
    {
      id: 3,
      node_run_id: 11,
      kind: "note",
      actor: "human",
      summary: "keep *this*",
      message: "keep *this* exactly",
      thinking: [],
      at: "",
    },
  ]);
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  expect(await screen.findByRole("heading", { name: "Grounding" })).toBeDefined();
  expect(screen.getByText("src/greet.sh").tagName).toBe("CODE");
  expect(screen.getByText("Checking baseline branch").tagName).toBe("STRONG");
  // Its lines are its lines: the title and the owner are not run together.
  expect(screen.getByText("PR2 - Show both forms").parentElement?.innerHTML).toBe(
    "<strong>PR2 - Show both forms</strong><br>Owner: frontend.",
  );
  expect(screen.queryByText(/\*\*|##/)).toBeNull();
  expect(screen.getByText("keep *this* exactly").tagName).toBe("P");
});

it("says who started a run, and only puts the operator's own words in their bubble", async () => {
  stub({ started_by: "watch", prompt: "Restack PR2 onto origin/main: PR1 was merged into main" });
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  const who = await screen.findByText("The pull-request watch started the run");
  const prompt = screen.getByText("Restack PR2 onto origin/main: PR1 was merged into main");
  expect(prompt.closest("article")).toBe(who.closest("article"));
  expect(who.closest("article")?.classList.contains("activity-message--human")).toBe(false);
});

it("puts a prompt the operator typed in their own bubble", async () => {
  stub({ started_by: "operator" });
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  const who = await screen.findByText("You started the run");
  expect(who.closest("article")?.classList.contains("activity-message--human")).toBe(true);
});

it("shows that a streamed tool call is still running while its result is pending", async () => {
  stub({}, false, [
    {
      id: 1,
      node_run_id: 11,
      kind: "tool_call",
      actor: "orchestrator",
      summary: "bash · npm test",
      message: null,
      thinking: [],
      at: "",
    },
  ]);
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  expect(await screen.findByText("bash · npm test")).toBeDefined();
  expect(screen.getByText("running…")).toBeDefined();
});

it("follows live activity and offers one-click return after the reader scrolls away", async () => {
  stub();
  const scrollTo = vi.fn();
  Object.defineProperty(HTMLElement.prototype, "scrollTo", {
    configurable: true,
    value: scrollTo,
  });
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  const feed = await screen.findByRole("log", { name: "orchestrator conversation" });
  await waitFor(() => expect(scrollTo).toHaveBeenCalled());

  Object.defineProperties(feed, {
    scrollHeight: { configurable: true, value: 1000 },
    clientHeight: { configurable: true, value: 200 },
    scrollTop: { configurable: true, value: 100 },
  });
  fireEvent.scroll(feed);
  await userEvent.click(screen.getByRole("button", { name: "Jump to live activity" }));
  expect(scrollTo).toHaveBeenLastCalledWith(expect.objectContaining({ top: 1000 }));
});

it("replies to the node in the selected run and checkout", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  const box = await screen.findByLabelText(/Reply to orchestrator/);
  await user.type(box, "Yes, continue with E5.3.");
  await user.click(screen.getByRole("button", { name: "Reply" }));

  await waitFor(() =>
    expect(calls.some((call) => call.path === "/runs/7/nodes/11/reply")).toBe(true),
  );
  const sent = calls.find((call) => call.path === "/runs/7/nodes/11/reply");
  expect(sent?.body).toEqual({ message: "Yes, continue with E5.3.", workspace: "/tmp/widget" });
  expect(await screen.findByText(/continue this conversation/)).toBeDefined();
});

it("reattaches an interrupted turn to its existing run, session, and checkout", async () => {
  const calls = stub({}, true);
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  expect(await screen.findByText("orchestrator was interrupted")).toBeDefined();
  await userEvent.click(screen.getByRole("button", { name: "Resume interrupted turn" }));
  await waitFor(() =>
    expect(calls.some((call) => call.path === "/runs/7/nodes/11/resume")).toBe(true),
  );
  expect(calls.find((call) => call.path === "/runs/7/nodes/11/resume")?.body).toEqual({
    workspace: "/tmp/widget",
  });
  expect(await screen.findByText(/resumed in the same run, session, and checkout/)).toBeDefined();
});

it("resumes an interrupted turn when the operator replies to it", async () => {
  const calls = stub({}, true);
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  const box = await screen.findByLabelText(/Reply to orchestrator/);
  await userEvent.type(box, "Continue from the existing diff.");
  await userEvent.click(screen.getByRole("button", { name: "Reply" }));

  await waitFor(() => {
    expect(calls.some((call) => call.path === "/runs/7/nodes/11/reply")).toBe(true);
    expect(calls.some((call) => call.path === "/runs/7/nodes/11/resume")).toBe(true);
  });
  expect(await screen.findByText(/resumed with your reply in the same session/)).toBeDefined();
});

it("approves the plan by continuing the selected run rather than starting another", async () => {
  const calls = stub({
    status: "blocked",
    plan_slug: "widget-plan",
    blocked_reason: "Plan ready for approval",
  });
  const held = {
    ...RUN,
    status: "blocked",
    plan_slug: "widget-plan",
    blocked_reason: "Plan ready for approval",
  };
  render(<AgentActivity runs={[held]} workspace="/tmp/widget" tick={0} />);

  await userEvent.click(await screen.findByRole("button", { name: "Approve plan and build" }));
  await waitFor(() =>
    expect(calls.some((call) => call.path === "/runs/7/approve-plan")).toBe(true),
  );
  expect(screen.getByText(/same run is continuing/i)).toBeDefined();
  expect(calls.some((call) => call.path === "/runs")).toBe(false);
});

it("can recover a run interrupted while it was preparing approval holds", async () => {
  const calls = stub({
    status: "blocked",
    plan_slug: "widget-plan",
    blocked_reason: "Preparing plan approval",
  });
  const preparing = {
    ...RUN,
    status: "blocked",
    plan_slug: "widget-plan",
    blocked_reason: "Preparing plan approval",
  };
  render(<AgentActivity runs={[preparing]} workspace="/tmp/widget" tick={0} />);

  await userEvent.click(await screen.findByRole("button", { name: "Approve plan and build" }));
  await waitFor(() =>
    expect(calls.some((call) => call.path === "/runs/7/approve-plan")).toBe(true),
  );
});

it("keeps concurrent agents in separate selectable conversations", async () => {
  const user = userEvent.setup();
  const calls = stub();
  render(<AgentActivity runs={[RUN]} workspace="/tmp/widget" tick={0} />);

  expect(await screen.findByText("I need an answer before I can continue.")).toBeDefined();
  expect(screen.queryByText("I am checking the visual state.")).toBeNull();
  // Each seat says what it is on: a PR's seats take several turns, and two that read
  // "frontend" cannot be told apart.
  expect(screen.getByRole("button", { name: "orchestrator" })).toBeDefined();
  await user.click(screen.getByRole("button", { name: "frontend W2 T2 try 2" }));
  expect(await screen.findByText("I am checking the visual state.")).toBeDefined();
  expect(screen.queryByText("I need an answer before I can continue.")).toBeNull();

  const box = screen.getByLabelText(/Reply to frontend/);
  await user.type(box, "Keep the smaller card.");
  await user.click(screen.getByRole("button", { name: "Reply" }));
  await waitFor(() =>
    expect(calls.some((call) => call.path === "/runs/7/nodes/12/reply")).toBe(true),
  );
});
