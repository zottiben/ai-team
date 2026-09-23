import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Board } from "./Board";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  localStorage.clear();
});

const PLAN = { plan: "widget", title: "Widget plan", status: "active", slice: null };

function slice(key: string, status: string, extra: Record<string, unknown> = {}) {
  return {
    id: Number(key.replace(/\D/g, "")) || 1,
    plan_id: 1,
    key,
    title: `${key} title`,
    status,
    ord: 10,
    scope_md: null,
    demo_md: null,
    estimate_files: null,
    branch: null,
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
      const request = {
        url: String(url),
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      };
      calls.push(request);
      const data = board as { slices?: ReturnType<typeof slice>[] };
      const key = decodeURIComponent(request.url.match(/\/board\/slices\/([^/?]+)/)?.[1] ?? "");
      const answer =
        request.method === "GET" && key !== ""
          ? { slice: data.slices?.find((candidate) => candidate.key === key), log: [] }
          : board;
      return Promise.resolve({ ok: true, json: async () => answer });
    }),
  );
  return calls;
}

it("shows every status and folds the usually empty columns", async () => {
  const user = userEvent.setup();
  stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready"), slice("S2", "done")] });
  render(<Board project="widget" tick={0} />);

  expect(await screen.findByText("Widget plan")).toBeDefined();
  expect(await screen.findByText("S1 title")).toBeDefined();
  // Done starts folded, as it does in ai-planner, but remains a real drop target.
  expect(screen.queryByText("S2 title")).toBeNull();
  await user.click(screen.getByLabelText("Expand Done"));
  expect(await screen.findByText("S2 title")).toBeDefined();

  for (const label of ["Draft", "Ready", "Active", "In review", "Blocked", "Done", "Deferred"]) {
    expect(screen.getByLabelText(new RegExp(`^${label},`))).toBeDefined();
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

  await user.click(await screen.findByText("S1 title"));
  await user.selectOptions(await screen.findByLabelText("Status"), "active");

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.url).toBe("/api/board/slices/S1");
  expect(write?.body).toMatchObject({ project: "widget", status: "active" });
});

it("dragging a card highlights the target and moves it through ai-planner", async () => {
  const calls = stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready")] });
  render(<Board project="widget" tick={0} />);

  const title = await screen.findByText("S1 title");
  const card = title.closest("article");
  const handle = card?.querySelector('[data-drag-key="S1"]');
  const target = screen.getByLabelText("Active, 0 slices");
  expect(handle).not.toBeNull();
  fireEvent.mouseDown(handle!, { button: 0, buttons: 1 });
  fireEvent.mouseMove(target, { buttons: 1 });
  expect(target.classList.contains("is-target")).toBe(true);
  fireEvent.mouseUp(target, { button: 0, buttons: 0 });

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  expect(calls.find((call) => call.method === "POST")?.body).toMatchObject({
    project: "widget",
    status: "active",
  });
});

it("persists a mouse-driven drop without relying on WebKit HTML drag events", async () => {
  const calls = stub({ plan: PLAN, next_step: null, slices: [slice("S1", "blocked")] });
  render(<Board project="widget" tick={0} />);

  const card = (await screen.findByText("S1 title")).closest("article");
  const handle = card?.querySelector('[data-drag-key="S1"]');
  const target = screen.getByLabelText("Ready, 0 slices");
  expect(card?.hasAttribute("draggable")).toBe(false);
  expect(handle?.hasAttribute("draggable")).toBe(false);
  fireEvent.mouseDown(handle!, { button: 0, buttons: 1 });
  fireEvent.mouseMove(target, { buttons: 1 });
  fireEvent.mouseUp(target, { button: 0, buttons: 0 });

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  expect(calls.find((call) => call.method === "POST")?.body).toMatchObject({
    project: "widget",
    status: "ready",
  });
});

it("does not let an older board read undo a persisted optimistic move", async () => {
  const blocked = { plan: PLAN, next_step: null, slices: [slice("S1", "blocked")] };
  const ready = { plan: PLAN, next_step: null, slices: [slice("S1", "ready")] };
  let getCount = 0;
  let resolveStale!: (response: unknown) => void;
  let resolvePost!: (response: unknown) => void;
  vi.stubGlobal(
    "fetch",
    vi.fn((_url: string, init?: RequestInit) => {
      if (init?.method === "POST") {
        return new Promise((resolve) => {
          resolvePost = resolve;
        });
      }
      getCount += 1;
      if (getCount === 2) {
        return new Promise((resolve) => {
          resolveStale = resolve;
        });
      }
      const body = getCount >= 3 ? ready : blocked;
      return Promise.resolve({ ok: true, json: async () => body });
    }),
  );

  const { rerender } = render(<Board project="widget" tick={0} />);
  const card = (await screen.findByText("S1 title")).closest("article");
  const handle = card?.querySelector('[data-drag-key="S1"]');
  rerender(<Board project="widget" tick={1} />);
  await waitFor(() => expect(getCount).toBe(2));

  const target = screen.getByLabelText("Ready, 0 slices");
  fireEvent.mouseDown(handle!, { button: 0, buttons: 1 });
  fireEvent.mouseMove(target, { buttons: 1 });
  fireEvent.mouseUp(target, { button: 0, buttons: 0 });
  expect(within(screen.getByLabelText("Ready, 1 slices")).getByText("S1 title")).toBeDefined();

  await act(async () => {
    resolveStale({ ok: true, json: async () => blocked });
  });
  expect(within(screen.getByLabelText("Ready, 1 slices")).getByText("S1 title")).toBeDefined();

  await act(async () => {
    resolvePost({ ok: true, json: async () => ({ moved: "S1" }) });
  });
  expect(within(screen.getByLabelText("Ready, 1 slices")).getByText("S1 title")).toBeDefined();
});

it("rolls an optimistic drag back when ai-planner refuses it", async () => {
  const data = { plan: PLAN, next_step: null, slices: [slice("S1", "ready")] };
  vi.stubGlobal(
    "fetch",
    vi.fn((_url: string, init?: RequestInit) =>
      Promise.resolve(
        init?.method === "POST"
          ? { ok: false, status: 409, json: async () => ({ error: "the plan changed underneath us" }) }
          : { ok: true, json: async () => data },
      ),
    ),
  );
  render(<Board project="widget" tick={0} />);
  const card = (await screen.findByText("S1 title")).closest("article");
  const handle = card?.querySelector('[data-drag-key="S1"]');
  const target = screen.getByLabelText("Active, 0 slices");
  fireEvent.mouseDown(handle!, { button: 0, buttons: 1 });
  fireEvent.mouseMove(target, { buttons: 1 });
  fireEvent.mouseUp(target, { button: 0, buttons: 0 });

  expect(await screen.findByText(/plan changed underneath us/)).toBeDefined();
  expect(within(screen.getByLabelText("Ready, 1 slices")).getByText("S1 title")).toBeDefined();
  expect(within(screen.getByLabelText("Active, 0 slices")).queryByText("S1 title")).toBeNull();
});

it("blocking a card carries the operator's reason, because the next session needs one", async () => {
  const user = userEvent.setup();
  vi.spyOn(window, "prompt").mockReturnValue("waiting for the API contract");
  const calls = stub({ plan: PLAN, next_step: null, slices: [slice("S1", "ready")] });
  render(<Board project="widget" tick={0} />);

  await user.click(await screen.findByText("S1 title"));
  await user.selectOptions(await screen.findByLabelText("Status"), "blocked");

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.body).toMatchObject({
    status: "blocked",
    reason: "waiting for the API contract",
  });
});

it("opens the scope, delivery and progress ai-planner records for a slice", async () => {
  const user = userEvent.setup();
  const item = slice("S1", "active", {
    scope_md: "Build the board.\n\nTouches: ui/**",
    demo_md: "Drag S1 to review.",
    branch: "feature/board",
    estimate_files: 6,
  });
  const data = { plan: PLAN, next_step: null, slices: [item] };
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      const request = {
        url: String(url),
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      };
      calls.push(request);
      const answer = request.url.includes("/board/slices/S1?")
        ? {
            slice: item,
            log: [
              {
                id: 1,
                plan_id: 1,
                slice_key: "S1",
                at: "2026-09-21T00:00:00Z",
                actor: "mcp",
                kind: "verification",
                branch: null,
                worktree_path: null,
                body: "The checks pass.",
              },
            ],
          }
        : data;
      return Promise.resolve({ ok: true, json: async () => answer });
    }),
  );
  render(<Board project="widget" tick={0} />);
  await user.click(await screen.findByText("S1 title"));

  const drawer = await screen.findByRole("dialog");
  expect(within(drawer).getByText(/Build the board/)).toBeDefined();
  expect(within(drawer).getByText("feature/board")).toBeDefined();
  expect(await within(drawer).findByText(/checks pass/)).toBeDefined();
  expect(within(drawer).getByText("Owned by backend")).toBeDefined();
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
