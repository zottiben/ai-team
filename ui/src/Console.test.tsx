import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Approvals, Prompt, Seats } from "./Console";
import type { Approval, NodeRun, RunDetail } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubPost() {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({ url: String(url), body: JSON.parse(String(init?.body ?? "null")) });
      return Promise.resolve({ ok: true, json: async () => ({ started: true, answered: true }) });
    }),
  );
  return calls;
}

const NODE: NodeRun = {
  id: 3,
  role: "backend",
  provider: "claude",
  model: "sonnet",
  status: "running",
  attempt: 1,
  slice_key: "S1",
  branch: null,
  blocked_reason: null,
};

it("starting a run sends the prompt for the selected project", async () => {
  const user = userEvent.setup();
  const calls = stubPost();
  const onStarted = vi.fn();

  render(<Prompt project="widget" onStarted={onStarted} />);
  await user.type(screen.getByLabelText("What should the team build?"), "add subtract");
  await user.click(screen.getByText("Start"));

  await waitFor(() => expect(onStarted).toHaveBeenCalled());
  expect(calls[0]?.url).toBe("/api/runs");
  expect(calls[0]?.body).toMatchObject({ project: "widget", prompt: "add subtract" });
});

it("an empty prompt means build what is already ready", async () => {
  // The same rule the CLI follows. Two surfaces disagreeing about what an empty prompt
  // means is worse than either behaviour on its own.
  const user = userEvent.setup();
  const calls = stubPost();

  render(<Prompt project="widget" onStarted={vi.fn()} />);
  await user.click(screen.getByText("Start"));

  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]?.body).toEqual({ project: "widget" });
});

it("refuses to start without a project rather than guessing one", async () => {
  const user = userEvent.setup();
  const calls = stubPost();

  render(<Prompt project={null} onStarted={vi.fn()} />);
  await user.click(screen.getByText("Start"));

  expect(await screen.findByText(/Pick a project first/)).toBeDefined();
  expect(calls).toHaveLength(0);
});

it("the org graph says who is working and on what", async () => {
  render(<Seats nodes={[NODE, { ...NODE, id: 4, role: "frontend", status: "done", attempt: 2 }]} />);

  expect(screen.getByText("backend")).toBeDefined();
  expect(screen.getByText("frontend")).toBeDefined();
  expect(screen.getAllByText("claude/sonnet").length).toBe(2);
  // A repeated attempt is worth seeing: it is the difference between slow and stuck.
  expect(screen.getByText(/try 2/)).toBeDefined();
});

it("says plainly when nothing has been dispatched", () => {
  render(<Seats nodes={[]} />);
  expect(screen.getByText(/No seat has been dispatched/)).toBeDefined();
});

it("answering a parked node sends the option it offered", async () => {
  const user = userEvent.setup();
  const calls = stubPost();
  const onAnswered = vi.fn();

  const run = { id: 7, nodes: [NODE] } as unknown as RunDetail;
  const pending: Approval[] = [
    {
      id: 11,
      node_run_id: 3,
      summary: "may I commit this?",
      payload: { request_id: "req_1", options: [{ id: "yes", label: "Approve" }] },
    },
  ];

  render(<Approvals run={run} pending={pending} onAnswered={onAnswered} />);
  expect(screen.getByText("may I commit this?")).toBeDefined();

  await user.click(screen.getByText("Approve"));
  await waitFor(() => expect(onAnswered).toHaveBeenCalled());

  expect(calls[0]?.url).toBe("/api/runs/7/approvals");
  // The node, not just the run: the question belongs to one turn.
  expect(calls[0]?.body).toEqual({ node: 3, request: "req_1", chose: "yes" });
});

it("shows nothing at all when nobody is waiting", () => {
  const run = { id: 7, nodes: [] } as unknown as RunDetail;
  const { container } = render(<Approvals run={run} pending={[]} onAnswered={vi.fn()} />);
  expect(container.firstChild).toBeNull();
});
