import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Talk } from "./Talk";
import type { Member } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function member(over: Partial<Member> = {}): Member {
  return {
    agent_id: 3,
    role: "backend",
    name: "Backend",
    provider: "claude",
    model: "sonnet",
    read_only: false,
    zone: "src/**",
    doing: "idle",
    node_run_id: null,
    run_id: null,
    slice_key: null,
    branch: null,
    attempt: 0,
    blocked_reason: null,
    last_said: null,
    reachable: false,
    turns: 0,
    tokens_in: 0,
    tokens_out: 0,
    ...over,
  };
}

function stub(reached: unknown) {
  const calls: { url: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      calls.push({
        url: String(input).replace(/^\/api/, ""),
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      return Promise.resolve({ ok: true, json: async () => reached });
    }),
  );
  return calls;
}

it("says the message will wait, before anything is typed", async () => {
  // These are different acts: one lands in the middle of a turn, the other starts minutes
  // of work. A box that silently did either would be a surprise waiting to happen.
  render(
    <Talk
      member={member({ doing: "working", reachable: true, slice_key: "S1" })}
      onClose={() => {}}
      onSent={() => {}}
    />,
  );
  expect(screen.getByText(/waits, and is the first thing it is given next/)).toBeDefined();
  expect(screen.getByText("Send")).toBeDefined();
});

it("says it will start a turn when the seat is idle", async () => {
  render(<Talk member={member({ doing: "idle" })} onClose={() => {}} onSent={() => {}} />);
  expect(screen.getByText(/starts a turn for it in its own worktree/)).toBeDefined();
  expect(screen.getByText("Start a turn")).toBeDefined();
});

it("a switched-off seat cannot be sent anything", async () => {
  // It would never be given the message, so offering to send one is a box that swallows it.
  const user = userEvent.setup();
  render(<Talk member={member({ doing: "disabled" })} onClose={() => {}} onSent={() => {}} />);

  await user.type(screen.getByLabelText("message for backend"), "hello");
  expect(screen.getByText("Start a turn").closest("button")?.disabled).toBe(true);
});

it("reports that a busy seat will get it next", async () => {
  const user = userEvent.setup();
  const calls = stub({ reached: "queued", node_run_id: 5, waiting: 1 });
  render(
    <Talk member={member({ doing: "working", reachable: true })} onClose={() => {}} onSent={() => {}} />,
  );

  await user.type(screen.getByLabelText("message for backend"), "use the new helper");
  await user.click(screen.getByText("Send"));

  await waitFor(() => expect(calls.length).toBeGreaterThan(0));
  expect(calls[0]?.url).toBe("/crew/3/say");
  expect(calls[0]?.body).toEqual({ message: "use the new helper" });
  expect(await screen.findByText(/gets this next/)).toBeDefined();
});

it("reports that a turn is starting, and where it will show up", async () => {
  // No run number, because the server does not have one yet - and an invented id reads as a
  // real one (M3-S17).
  const user = userEvent.setup();
  stub({ reached: "started" });
  render(<Talk member={member()} onClose={() => {}} onSent={() => {}} />);

  await user.type(screen.getByLabelText("message for backend"), "fix the failing test");
  await user.click(screen.getByText("Start a turn"));

  expect(await screen.findByText(/Starting a turn for Backend/)).toBeDefined();
  expect(screen.getByText(/appear in the runs below/)).toBeDefined();
});

it("passes on a refusal in the server's own words", async () => {
  const user = userEvent.setup();
  stub({ reached: "refused", because: "backend is switched off" });
  render(<Talk member={member({ doing: "idle" })} onClose={() => {}} onSent={() => {}} />);

  await user.type(screen.getByLabelText("message for backend"), "anything");
  await user.click(screen.getByText("Start a turn"));
  expect(await screen.findByText("backend is switched off")).toBeDefined();
});

it("a process that has gone since the panel opened says what to do", async () => {
  // The liveness claim is worth trying, not proven: it can be a moment stale.
  const user = userEvent.setup();
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 400,
      json: async () => ({
        error: "backend was working a moment ago but its process has gone - say it again to start a fresh turn",
      }),
    }),
  );
  render(
    <Talk member={member({ doing: "working", reachable: true })} onClose={() => {}} onSent={() => {}} />,
  );

  await user.type(screen.getByLabelText("message for backend"), "hello");
  await user.click(screen.getByText("Send"));
  expect(await screen.findByText(/say it again to start a fresh turn/)).toBeDefined();
});

it("an empty message sends nothing", async () => {
  const user = userEvent.setup();
  const calls = stub({ reached: "started" });
  render(<Talk member={member()} onClose={() => {}} onSent={() => {}} />);

  await user.type(screen.getByLabelText("message for backend"), "   ");
  expect(screen.getByText("Start a turn").closest("button")?.disabled).toBe(true);
  expect(calls).toHaveLength(0);
});

it("the chord sends and Enter is a newline", async () => {
  // The other way round sends half-written instructions to an agent.
  const user = userEvent.setup();
  const calls = stub({ reached: "started" });
  render(<Talk member={member()} onClose={() => {}} onSent={() => {}} />);

  const box = screen.getByLabelText("message for backend");
  await user.type(box, "first line{Enter}second line");
  expect(calls).toHaveLength(0);
  expect((box as HTMLTextAreaElement).value).toContain("\n");

  await user.type(box, "{Control>}{Enter}{/Control}");
  await waitFor(() => expect(calls.length).toBeGreaterThan(0));
});
