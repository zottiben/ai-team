import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Board } from "./Board";

afterEach(() => {
  vi.unstubAllGlobals();
});

const PLAN = { plan: "widget", title: "Widget plan", status: "active", slice: null };

function slice(key: string, status: string, extra: Record<string, unknown> = {}) {
  return {
    key,
    title: `${key} title`,
    status,
    ord: 10,
    scope_md: null,
    demo_md: null,
    claimed_by: null,
    owner: "backend",
    touches: ["crates/**"],
    ...extra,
  };
}

/** Records every request so a test can assert what was written back. */
function stub(board: unknown) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({
        url: String(url),
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      return Promise.resolve({ ok: true, json: async () => board });
    }),
  );
  return calls;
}

it("shows the plan's slices in the columns work moves through", async () => {
  stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready"), slice("S2", "done")] });
  render(<Board project="widget" tick={0} />);

  expect(await screen.findByText("Widget plan")).toBeDefined();
  expect(await screen.findByText("S1 title")).toBeDefined();
  expect(await screen.findByText("S2 title")).toBeDefined();

  // Every column is present even when empty: the columns are the shape of the workflow,
  // and a board that reflows as work moves has to be re-read every time.
  for (const column of ["draft", "ready", "active", "in review", "blocked", "done", "deferred"]) {
    expect(screen.getAllByText(column).length).toBeGreaterThan(0);
  }
});

it("says which seat would build each slice", async () => {
  // ai-team's own question, not the plan's: which zone covers the paths it declared.
  stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready")] });
  render(<Board project="widget" tick={0} />);
  expect(await screen.findByText("backend")).toBeDefined();
});

it("says plainly when nobody owns a slice", async () => {
  // A slice no zone covers is reported undone rather than handed to somebody, so the
  // board should not imply it is ready to go.
  stub({ plan: PLAN, next_step: null, slices: [slice("S9", "ready", { owner: null, touches: ["docs/x.md"] })] });
  render(<Board project="widget" tick={0} />);
  expect(await screen.findByText("unowned")).toBeDefined();
});

it("moving a card writes back through ai-planner", async () => {
  const user = userEvent.setup();
  const calls = stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready")] });
  render(<Board project="widget" tick={0} />);

  await screen.findByText("S1 title");
  await user.selectOptions(screen.getByLabelText("move S1"), "active");

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.url).toBe("/api/board/slices/S1");
  expect(write?.body).toMatchObject({ project: "widget", status: "active" });
});

it("blocking a card carries a reason, because the next session needs one", async () => {
  const user = userEvent.setup();
  const calls = stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready")] });
  render(<Board project="widget" tick={0} />);

  await screen.findByText("S1 title");
  await user.selectOptions(screen.getByLabelText("move S1"), "blocked");

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.body).toMatchObject({ status: "blocked" });
  expect((write?.body as { reason?: string }).reason).toBeTruthy();
});

it("says what is wrong rather than showing an empty board", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 500,
      json: async () => ({ error: "that project has no checkout, so it has no plan to show" }),
    }),
  );
  render(<Board project="widget" tick={0} />);
  expect(await screen.findByText(/no checkout/)).toBeDefined();
});

it("a checkout with no plan is told what to do, not shown an error", async () => {
  // Where every new project starts. Answering with ai-planner's own "not registered"
  // made a first look at the Board a red message about a tool the reader may not have met.
  stub({ plan: null, slices: [], next_step: "Run `aip new` in it." });
  render(<Board project="widget" tick={0} />);
  expect(await screen.findByText(/aip new/)).toBeDefined();
});

it("asks for a project before reading anything", async () => {
  const calls = stub({ plan: PLAN, next_step: null, slices: [] });
  render(<Board project={null} tick={0} />);
  expect(screen.getByText(/Pick a project/)).toBeDefined();
  expect(calls).toHaveLength(0);
});
