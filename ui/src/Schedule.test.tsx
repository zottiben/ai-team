import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Schedule } from "./Schedule";
import type { ChatSchedule, Reminder, ScheduleOccurrence } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function reminder(over: Partial<Reminder>): Reminder {
  return {
    id: 1,
    project_id: 1,
    chat_id: null,
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

function occurrence(over: Partial<ScheduleOccurrence>): ScheduleOccurrence {
  return {
    id: 1,
    occurrence_at: "2026-09-18T09:00:00Z",
    claimed_at: "2026-09-18T09:00:04Z",
    skipped: 0,
    outcome: "started",
    detail: "Started in this chat.",
    run_id: 4,
    node_id: 9,
    settled_at: "2026-09-18T09:00:04Z",
    ...over,
  };
}

function schedule(over: Partial<ChatSchedule>): ChatSchedule {
  return {
    id: 1,
    reminder_id: 7,
    chat_id: 3,
    project_id: 1,
    workspace_path: "/repo/widget",
    prompt: "tidy the flaky tests",
    provider: "local",
    model: "fixture-model",
    mode: "single",
    reminder: reminder({ id: 7, kind: "scheduled_run", chat_id: 3 }),
    chat_title: "Flaky tests",
    project_slug: "widget",
    occurrences: [],
    ...over,
  };
}

function stub(rows: Reminder[], schedules: ChatSchedule[] = []) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({
        url: String(url).replace(/^\/api/, ""),
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      const path = String(url).replace(/^\/api/, "");
      // Routed rather than blanket: the form asks which project and which chat a
      // scheduled run belongs to, so each request has to answer with its own subject.
      if (path === "/projects") {
        return Promise.resolve({
          ok: true,
          json: async () => [
            { id: 1, slug: "widget", name: "Widget", kind: "repo", status: "active", open_runs: 0 },
          ],
        });
      }
      if (path === "/chat-schedules") {
        return Promise.resolve({ ok: true, json: async () => schedules });
      }
      if (path.startsWith("/chats?")) {
        return Promise.resolve({
          ok: true,
          json: async () => [
            { id: 3, project_id: 1, title: "Flaky tests", archived: false },
            { id: 4, project_id: 1, title: "Old and filed", archived: true },
          ],
        });
      }
      return Promise.resolve({ ok: true, json: async () => (init?.method ? rows[0] : rows) });
    }),
  );
  return calls;
}

it("keeps a fired one-shot schedule and its result reachable after reload", async () => {
  const fired = reminder({ id: 7, kind: "scheduled_run", chat_id: 3, status: "fired", title: "Finished nightly" });
  stub([fired], [schedule({ reminder: fired, occurrences: [occurrence({})] })]);
  const onOpenChat = vi.fn();
  render(<Schedule tick={0} onOpenChat={onOpenChat} />);
  await screen.findByText("Finished nightly");
  await userEvent.click(screen.getByRole("button", { name: "Open Flaky tests" }));
  expect(onOpenChat).toHaveBeenCalledWith("widget", 3);
  expect(screen.getByText("Started in this chat.")).toBeDefined();
  expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
});

/** Fill in everything a scheduled chat run needs, in the order the form asks for it. */
async function scheduleRun(
  user: ReturnType<typeof userEvent.setup>,
  { chat = true, prompt = "tidy the flaky tests" } = {},
) {
  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await user.type(screen.getByLabelText("title"), "nightly build");
  await user.type(screen.getByLabelText("when"), "2h");
  await waitFor(() => expect(screen.getByRole("option", { name: "Widget" })).toBeDefined());
  await user.selectOptions(screen.getByLabelText("project"), "widget");
  if (chat) {
    await waitFor(() => expect(screen.getByRole("option", { name: "Flaky tests" })).toBeDefined());
    await user.selectOptions(screen.getByLabelText("chat"), "3");
  }
  if (prompt !== "") await user.type(screen.getByLabelText("prompt"), prompt);
  await user.click(screen.getByText("Add"));
}

it("an idea needs no date, which is what makes it an inbox", async () => {
  const user = userEvent.setup();
  const calls = stub([reminder({ kind: "idea", due_at: null })]);
  render(<Schedule tick={0} />);

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
  render(<Schedule tick={0} />);

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
  render(<Schedule tick={0} />);

  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await user.type(screen.getByLabelText("title"), "nightly build");
  await user.type(screen.getByLabelText("when"), "2");
  // Waits for the option, not just the picker: the projects arrive on their own request.
  await waitFor(() => expect(screen.getByRole("option", { name: "Widget" })).toBeDefined());
  await user.selectOptions(screen.getByLabelText("project"), "widget");
  await waitFor(() => expect(screen.getByRole("option", { name: "Flaky tests" })).toBeDefined());
  await user.selectOptions(screen.getByLabelText("chat"), "3");
  await user.type(screen.getByLabelText("prompt"), "tidy the flaky tests");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  const due = (calls.find((call) => call.method === "POST")?.body as { due_at: string }).due_at;
  const delta = new Date(due).getTime() - before;
  expect(delta).toBeGreaterThan(100_000); // two minutes, not two seconds
  expect(delta).toBeLessThan(130_000);
});

it("scheduled work names the exact chat it will happen in, and what to send it", async () => {
  // Not the project's latest run and not a new conversation: a person reads the result
  // where they asked for the work, and the prompt is theirs rather than inferred.
  const user = userEvent.setup();
  const calls = stub([]);
  render(<Schedule tick={0} />);

  await scheduleRun(user);

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  expect(calls.find((call) => call.method === "POST")?.body).toMatchObject({
    kind: "scheduled_run",
    project: "widget",
    chat: 3,
    prompt: "tidy the flaky tests",
  });
});

it("an archived chat is not offered, because nothing can run in one", async () => {
  const user = userEvent.setup();
  stub([]);
  render(<Schedule tick={0} />);

  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await waitFor(() => expect(screen.getByRole("option", { name: "Widget" })).toBeDefined());
  await user.selectOptions(screen.getByLabelText("project"), "widget");

  await waitFor(() => expect(screen.getByRole("option", { name: "Flaky tests" })).toBeDefined());
  expect(screen.queryByRole("option", { name: "Old and filed" })).toBeNull();
});

it("refuses scheduled work with no chat and no prompt, rather than guessing either", async () => {
  const user = userEvent.setup();
  const calls = stub([]);
  render(<Schedule tick={0} />);

  await scheduleRun(user, { chat: false, prompt: "" });
  expect(await screen.findByText(/Which chat/)).toBeDefined();
  expect(calls.some((call) => call.method === "POST")).toBe(false);

  await user.selectOptions(screen.getByLabelText("chat"), "3");
  await user.click(screen.getByText("Add"));
  expect(await screen.findByText(/needs its own prompt/)).toBeDefined();
  expect(calls.some((call) => call.method === "POST")).toBe(false);
});

it("shows where scheduled work went, and opens that exact chat", async () => {
  const user = userEvent.setup();
  const opened: [string, number][] = [];
  stub(
    [reminder({ id: 7, kind: "scheduled_run", chat_id: 3, title: "nightly sweep" })],
    [schedule({ occurrences: [occurrence({})] })],
  );
  render(<Schedule tick={0} onOpenChat={(project, chat) => opened.push([project, chat])} />);

  expect(await screen.findByText("tidy the flaky tests")).toBeDefined();
  expect(screen.getByText(/Started in this chat/)).toBeDefined();
  await user.click(screen.getByText("Open Flaky tests"));
  expect(opened).toEqual([["widget", 3]]);
});

it("says when an occurrence was skipped, missed or refused, and never retries it", async () => {
  // A nightly that quietly did nothing for a fortnight is the failure this page exists
  // to make impossible to miss.
  stub(
    [reminder({ id: 7, kind: "scheduled_run", chat_id: 3, title: "nightly sweep" })],
    [
      schedule({
        occurrences: [
          occurrence({
            id: 2,
            outcome: "busy",
            detail: "The chat was already working when this was due, so it was skipped.",
            skipped: 3,
            run_id: null,
            node_id: null,
          }),
          occurrence({
            id: 1,
            outcome: "refused",
            detail: "This chat's execution mode changed after this schedule was made.",
            run_id: null,
            node_id: null,
          }),
        ],
      }),
    ],
  );
  render(<Schedule tick={0} />);

  expect(await screen.findByText(/3 earlier occurrences were missed/)).toBeDefined();
  expect(screen.getByText(/execution mode changed/)).toBeDefined();
  expect(screen.getByText("busy")).toBeDefined();
  expect(screen.getByText("refused")).toBeDefined();
});

it("a scheduled run is refused until it has somewhere to run", async () => {
  // Inheriting whichever project happened to be selected elsewhere would schedule work in
  // the wrong repository, which is worse than being asked (D18).
  const user = userEvent.setup();
  const calls = stub([]);
  render(<Schedule tick={0} />);

  await user.selectOptions(await screen.findByLabelText("kind"), "scheduled_run");
  await user.type(screen.getByLabelText("title"), "nightly build");
  await user.type(screen.getByLabelText("when"), "2h");
  await user.click(screen.getByText("Add"));

  expect(await screen.findByText(/Which project/)).toBeDefined();
  expect(calls.some((call) => call.method === "POST")).toBe(false);
});

it("a reminder needs no project, because it belongs to whoever is reading it", async () => {
  const user = userEvent.setup();
  const calls = stub([]);
  render(<Schedule tick={0} />);

  await user.type(await screen.findByLabelText("title"), "cut the release");
  await user.type(screen.getByLabelText("when"), "2h");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((call) => call.method === "POST")).toBe(true));
  expect(screen.queryByLabelText("project")).toBeNull();
});

it("says the clock needs a process, because a run that never fires looks like a bug", async () => {
  stub([]);
  render(<Schedule tick={0} />);
  expect(await screen.findByText(/one of\s+them needs to be up/)).toBeDefined();
});

it("shows what is scheduled and cancels it", async () => {
  const user = userEvent.setup();
  const calls = stub([reminder({ id: 7, kind: "scheduled_run", title: "nightly build" })]);
  render(<Schedule tick={0} />);

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
  render(<Schedule tick={0} />);

  expect(await screen.findByText("still to come")).toBeDefined();
  expect(screen.queryByText("already went off")).toBeNull();
});

it("says plainly when there is nothing at all", async () => {
  stub([]);
  render(<Schedule tick={0} />);
  expect(await screen.findByText(/no ideas yet/)).toBeDefined();
});
