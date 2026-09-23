import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Prompt, Seats } from "./Console";
import type { NodeRun, Worktree } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubPost() {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({ url: String(url), body: JSON.parse(String(init?.body ?? "null")) });
      return Promise.resolve({ ok: true, json: async () => ({ started: true, run_id: 19 }) });
    }),
  );
  return calls;
}

const TASK: Worktree = {
  name: "1",
  path: "/tmp/widget-task",
  status: "in-use",
  lease_holder: null,
  processes: [],
  branch: "feature/task",
  main: false,
};

const NODE: NodeRun = {
  id: 3,
  role: "backend",
  provider: "claude",
  model: "sonnet",
  status: "running",
  attempt: 1,
  slice_key: "S1",
  worktree_path: "/tmp/widget",
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

  await waitFor(() => expect(onStarted).toHaveBeenCalledWith(19));
  expect((await screen.findByRole("status")).textContent).toContain("Run #19 started");
  expect(calls[0]?.url).toBe("/api/runs");
  expect(calls[0]?.body).toMatchObject({ project: "widget", prompt: "add subtract" });
});

it("starts the full team workflow inside the selected worktree", async () => {
  const user = userEvent.setup();
  const calls = stubPost();

  render(<Prompt project="widget" workspace={TASK} onStarted={vi.fn()} />);
  await user.type(screen.getByLabelText("What should the team build?"), "finish this task");
  await user.click(screen.getByText("Start"));

  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]?.body).toEqual({
    project: "widget",
    workspace: "/tmp/widget-task",
    prompt: "finish this task",
    approval_required: true,
  });
  expect(screen.getByText(/runs in this checkout/)).toBeDefined();
});

it("works on the default branch only when asked, and only for that run", async () => {
  const user = userEvent.setup();
  const calls = stubPost();

  render(<Prompt project="widget" onStarted={vi.fn()} />);
  const option = screen.getByLabelText("Work on the default branch itself");
  expect((option as HTMLInputElement).checked).toBe(false);

  await user.type(screen.getByLabelText("What should the team build?"), "first");
  await user.click(screen.getByText("Start"));
  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]?.body).not.toHaveProperty("branching");

  await user.click(option);
  await user.type(screen.getByLabelText("What should the team build?"), "second");
  await user.click(screen.getByText("Start"));
  await waitFor(() => expect(calls).toHaveLength(2));
  expect(calls[1]?.body).toMatchObject({ prompt: "second", branching: "default_branch" });
  // Asked for one run, so it does not carry over to the next.
  expect((option as HTMLInputElement).checked).toBe(false);
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

it("names the task a seat built, not only its pull request", () => {
  render(<Seats nodes={[{ ...NODE, slice_key: "PR1", task_key: "T2" }, NODE]} />);
  expect(screen.getByText("PR1 T2")).toBeTruthy();
  // A pull request built as one piece of work reads as it always did.
  expect(screen.getByText("S1")).toBeTruthy();
});

it("says plainly when nothing has been dispatched", () => {
  render(<Seats nodes={[]} />);
  expect(screen.getByText(/No seat has been dispatched/)).toBeDefined();
});
