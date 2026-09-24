import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Today } from "./Today";
import type { TodayItem } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function item(over: Partial<TodayItem>): TodayItem {
  return {
    urgency: "review",
    kind: "review",
    title: "something",
    detail: null,
    project: "widget",
    run_id: null,
    since: null,
    ...over,
  };
}

function stub(
  items: TodayItem[],
  support: { projects?: unknown[]; runs?: unknown[]; analytics?: unknown[] } = {},
) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const url = String(input).replace(/^\/api/, "");
      const body = url === "/today"
        ? items
        : url === "/projects"
          ? (support.projects ?? [])
          : url.startsWith("/runs")
            ? (support.runs ?? [])
            : url.startsWith("/analytics")
              ? (support.analytics ?? [])
              : [];
      return Promise.resolve({ ok: true, json: async () => body });
    }),
  );
}

it("calls out the first item rather than leaving it to look like the rest", async () => {
  // A ranked list whose top item renders identically to the others is a list people read
  // top to bottom anyway, which wastes the ranking.
  stub([
    item({ urgency: "blocking", kind: "approval", title: "may I commit?", run_id: 7 }),
    item({ title: "PR1" }),
  ]);
  render(<Today tick={0} onOpenRun={() => {}} />);

  expect(await screen.findByText("Do this first")).toBeDefined();
  expect(await screen.findByText("may I commit?")).toBeDefined();
  expect(await screen.findByText(/a run is parked and you are the reason/)).toBeDefined();
});

it("keeps the server's order instead of re-sorting it", async () => {
  // The ranking is one judgement, tested in core. A second one here would be a second
  // answer to the same question.
  stub([
    item({ urgency: "blocking", title: "first" }),
    item({ urgency: "failed", title: "second" }),
    item({ urgency: "in_flight", title: "third" }),
  ]);
  const { container } = render(<Today tick={0} onOpenRun={() => {}} />);
  await screen.findByText("first");

  const titles = [...container.querySelectorAll(".today-row")].map(
    (card) => card.querySelector("strong")?.textContent,
  );
  expect(titles).toEqual(["first", "second", "third"]);
});

it("an item pointing at a run can be followed; one that does not, cannot", async () => {
  // A button that does nothing is worse than plain text: it invites a click and then
  // ignores it.
  const user = userEvent.setup();
  const opened: number[] = [];
  stub([
    item({ urgency: "blocking", title: "answer this", run_id: 7 }),
    item({ urgency: "due", kind: "reminder", title: "stand-up", run_id: null }),
  ]);
  render(<Today tick={0} onOpenRun={(id) => opened.push(id)} />);

  await user.click(await screen.findByText("answer this"));
  expect(opened).toEqual([7]);

  expect(screen.getByText("stand-up").closest("button")).toBeNull();
});

it("a review waiting on you opens that review, not the run that built it", async () => {
  // Following it used to open the run - a page of agent activity - and the review had to
  // be found again under Review.
  const user = userEvent.setup();
  const runs: number[] = [];
  const reviews: Array<[number, string | null, number | null]> = [];
  stub([
    item({ urgency: "review", kind: "review", title: "PR1: subtract", run_id: 7, review_id: 3 }),
  ]);
  render(
    <Today
      tick={0}
      onOpenRun={(id) => runs.push(id)}
      onOpenReview={(id, project, run) => reviews.push([id, project, run])}
    />,
  );

  await user.click(await screen.findByText("PR1: subtract"));
  expect(reviews).toEqual([[3, "widget", 7]]);
  expect(runs).toEqual([]);
});

it("says plainly when nothing is waiting", async () => {
  stub([]);
  render(<Today tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText(/Nothing is waiting on you/)).toBeDefined();
});

it("shows live operations without changing the ranked queue", async () => {
  stub(
    [
      item({ urgency: "failed", title: "fix gates", project: "widget" }),
      item({ urgency: "review", title: "review it", project: "widget" }),
      item({ urgency: "in_flight", title: "backend is building", project: "widget" }),
    ],
    {
      projects: [
        { id: 1, slug: "widget", name: "Widget", kind: "repo", status: "active", open_runs: 1 },
      ],
      runs: [
        { id: 9, project_id: 1, prompt: "build", status: "running", trigger: "manual", created_at: "now", started_at: "now", ended_at: null },
      ],
      analytics: [
        { attempts: 10, accepted: 8, gates_run: 5, gates_passed: 4 },
      ],
    },
  );
  render(<Today tick={0} onOpenRun={() => {}} />);

  expect(await screen.findByText("Personal operations")).toBeDefined();
  expect(screen.getByText("Agents working").previousElementSibling?.textContent).toBe("1");
  expect(screen.getByText("Active runs").previousElementSibling?.textContent).toBe("1");
  expect(screen.getByText("Widget")).toBeDefined();
  expect(screen.getByText("Recent activity")).toBeDefined();
  expect(screen.getByText("build")).toBeDefined();
  expect(screen.getAllByText("80%").length).toBe(2);
});

it("labels every tier with a word, not only a colour", async () => {
  // There is no browser on the machine this is built on, and a colour alone is
  // unreadable to anyone who cannot tell two of them apart.
  stub([
    item({ urgency: "blocking", title: "a" }),
    item({ urgency: "overdue", title: "b" }),
    item({ urgency: "failed", title: "c" }),
    item({ urgency: "review", title: "d" }),
    item({ urgency: "question", title: "e" }),
    item({ urgency: "due", title: "f" }),
    item({ urgency: "in_flight", title: "g" }),
  ]);
  render(<Today tick={0} onOpenRun={() => {}} />);

  for (const label of ["Blocking", "Overdue", "Failed", "Review", "Question", "Due", "In flight"]) {
    expect((await screen.findAllByText(label)).length).toBeGreaterThan(0);
  }
});

it("says what is wrong rather than showing an empty day", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      json: async () => ({ error: "no database yet - run `ait init`" }),
    }),
  );
  render(<Today tick={0} onOpenRun={() => {}} />);
  expect(await screen.findByText(/ait init/)).toBeDefined();
});
