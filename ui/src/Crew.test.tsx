import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Crew } from "./Crew";
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

function stub(crew: Member[]) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      calls.push(String(input).replace(/^\/api/, ""));
      return Promise.resolve({ ok: true, json: async () => crew });
    }),
  );
  return calls;
}

it("asks once for the whole crew", async () => {
  // The window polls; six requests a tick would be five too many, and a seat arriving a tick
  // after its neighbour makes the panel look unstable.
  const calls = stub([member()]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  await waitFor(() => expect(calls.length).toBeGreaterThan(0));
  expect(calls.filter((url) => url.startsWith("/crew")).length).toBe(1);
});

it("shows every seat, including the ones doing nothing", async () => {
  // Hiding the idle ones makes "where is the frontend" a question the dashboard created.
  stub([
    member({ agent_id: 1, role: "backend", name: "Backend", doing: "working", slice_key: "S1" }),
    member({ agent_id: 2, role: "frontend", name: "Frontend", doing: "untouched" }),
  ]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);

  expect(await screen.findByText("Backend")).toBeDefined();
  expect(screen.getByText("Frontend")).toBeDefined();
  expect(screen.getByText("not started")).toBeDefined();
});

it("summarises how many are working and how many are waiting on you", async () => {
  stub([
    member({ agent_id: 1, doing: "working" }),
    member({ agent_id: 2, doing: "parked" }),
    member({ agent_id: 3, doing: "idle" }),
  ]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText(/1 working, 1 waiting on you/)).toBeDefined();
});

it("says which slice a working seat is on", async () => {
  stub([member({ doing: "working", slice_key: "S1", run_id: 7 })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("S1")).toBeDefined();
  expect(screen.getByText("working")).toBeDefined();
});

it("calls out a retry, because that reads differently from progress", async () => {
  // A second attempt means the first was rejected.
  stub([member({ doing: "working", slice_key: "S1", attempt: 2 })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("attempt 2")).toBeDefined();
});

it("why a seat stopped beats a colour saying that it did", async () => {
  stub([member({ doing: "failed", slice_key: "S1", blocked_reason: "gates rejected it twice" })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("gates rejected it twice")).toBeDefined();
});

it("carries the last thing a seat said, so the card is worth reading", async () => {
  stub([member({ doing: "idle", last_said: "added the subtract function" })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("added the subtract function")).toBeDefined();
});

it("a failure shows its reason rather than its last remark", async () => {
  // Both would be two explanations of the same thing, and the reason is the one that matters.
  stub([
    member({ doing: "failed", blocked_reason: "gates rejected it", last_said: "I think that's done" }),
  ]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("gates rejected it")).toBeDefined();
  expect(screen.queryByText("I think that's done")).toBeNull();
});

it("a seat with a run can open it", async () => {
  const user = userEvent.setup();
  stub([member({ doing: "working", run_id: 7 })]);
  const opened: number[] = [];
  render(<Crew project="widget" tick={0} onOpenRun={(id) => opened.push(id)} />);

  await user.click(await screen.findByText("Its run"));
  expect(opened).toEqual([7]);
});

it("a seat that has never run offers nothing to open", async () => {
  // A button that does nothing invites a click and then ignores it.
  stub([member({ doing: "untouched", run_id: null })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  await screen.findByText("Backend");
  expect(screen.queryByText("Its run")).toBeNull();
});

it("a disabled seat says it will not be given work", async () => {
  // Which is different from having none.
  stub([member({ doing: "disabled" })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText("off")).toBeDefined();
  expect(screen.getByText(/will not be given work/)).toBeDefined();
});

it("shows what a seat has sent, as the rate-limit number", async () => {
  stub([member({ doing: "idle", tokens_in: 197_576, tokens_out: 2_176 })]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  // Waited for, because the card renders after the request resolves.
  expect(await screen.findByText(/198k in · 2k out/)).toBeDefined();
});

it("says plainly when a project has no team", async () => {
  stub([]);
  render(<Crew project="widget" tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText(/no team yet/)).toBeDefined();
});
