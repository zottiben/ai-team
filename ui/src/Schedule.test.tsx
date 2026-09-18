import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Schedule } from "./Schedule";
import type { Reminder } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function reminder(over: Partial<Reminder>): Reminder {
  return {
    id: 1,
    project_id: 1,
    kind: "reminder",
    title: "stand-up",
    body: "",
    prompt: null,
    due_at: "2026-09-19T09:00:00Z",
    recur: null,
    status: "pending",
    last_fired_at: null,
    ...over,
  };
}

function stub(rows: Reminder[]) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({
        url: String(url).replace(/^\/api/, ""),
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      return Promise.resolve({ ok: true, json: async () => (init?.method ? rows[0] : rows) });
    }),
  );
  return calls;
}

it("an idea needs no date, which is what makes it an inbox", async () => {
  const user = userEvent.setup();
  const calls = stub([reminder({ kind: "idea", due_at: null })]);
  render(<Schedule project="widget" tick={0} />);

  await user.type(await screen.findByLabelText("title"), "what if runs could fork");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const write = calls.find((call) => call.method === "POST");
  expect(write?.body).toMatchObject({ kind: "idea", title: "what if runs could fork" });
  expect((write?.body as { due_at?: string }).due_at).toBeUndefined();
});

it("anything that is not an idea must say when", async () => {
  // Otherwise it sits there never firing, and looks like a broken scheduler.
  const user = userEvent.setup();
  const calls = stub([]);
  render(<Schedule project="widget" tick={0} />);

  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await user.type(screen.getByLabelText("title"), "nightly build");
  await user.click(screen.getByText("Add"));

  expect(await screen.findByText(/When\?/)).toBeDefined();
  expect(calls.some((call) => call.method === "POST")).toBe(false);
});

it("a bare number is minutes, because that is what people mean", async () => {
  // Reading `2` as two seconds would fire a scheduled run almost immediately.
  const user = userEvent.setup();
  const calls = stub([]);
  const before = Date.now();
  render(<Schedule project="widget" tick={0} />);

  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await user.type(screen.getByLabelText("title"), "nightly build");
  await user.type(screen.getByLabelText("when"), "2");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const due = (calls.find((call) => call.method === "POST")?.body as { due_at: string }).due_at;
  const delta = new Date(due).getTime() - before;
  expect(delta).toBeGreaterThan(100_000); // two minutes, not two seconds
  expect(delta).toBeLessThan(130_000);
});

it("says the clock needs a process, because a run that never fires looks like a bug", async () => {
  stub([]);
  render(<Schedule project={null} tick={0} />);
  expect(await screen.findByText(/one of\s+them needs to be up/)).toBeDefined();
});

it("shows what is scheduled and cancels it", async () => {
  const user = userEvent.setup();
  const calls = stub([reminder({ id: 7, kind: "scheduled_run", title: "nightly build" })]);
  render(<Schedule project="widget" tick={0} />);

  expect(await screen.findByText("nightly build")).toBeDefined();
  await user.click(screen.getByText("Cancel"));
  await waitFor(() =>
    expect(calls.some((call) => call.url === "/reminders/7" && call.method === "DELETE")).toBe(
      true,
    ),
  );
});

it("a fired one-off drops off the list rather than lingering", async () => {
  stub([
    reminder({ id: 1, title: "already went off", status: "fired" }),
    reminder({ id: 2, title: "still to come", status: "pending" }),
  ]);
  render(<Schedule project="widget" tick={0} />);

  expect(await screen.findByText("still to come")).toBeDefined();
  expect(screen.queryByText("already went off")).toBeNull();
});

it("says plainly when there is nothing at all", async () => {
  stub([]);
  render(<Schedule project="widget" tick={0} />);
  expect(await screen.findByText(/no ideas yet/)).toBeDefined();
});
