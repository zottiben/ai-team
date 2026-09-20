import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Analytics } from "./Analytics";
import type { AnalyticsRow } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function row(over: Partial<AnalyticsRow>): AnalyticsRow {
  return {
    group: "backend",
    attempts: 4,
    accepted: 3,
    rejected: 1,
    slices_accepted: 3,
    tokens_in: 1_000,
    tokens_out: 900,
    cache_read: 60_000,
    cache_write: 4_000,
    seconds: 600,
    cycle_seconds: 1_800,
    gates_run: 5,
    gates_passed: 4,
    accepted_rate: 0.75,
    rework: 1.33,
    total_input: 65_000,
    input_per_accepted: 21_667,
    cache_hit_rate: 0.94,
    yield_per_k: 13.8,
    gate_pass_rate: 0.8,
    cycle_time: 600,
    ...over,
  };
}

function stub(rows: AnalyticsRow[]) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(String(url));
      return Promise.resolve({ ok: true, json: async () => rows });
    }),
  );
  return calls;
}

it("shows what a pairing lands and what it costs in context to land it", async () => {
  stub([row({})]);
  render(<Analytics tick={0} />);

  const line = within(await screen.findByRole("row", { name: /backend/ }));
  expect(line.getByText("75%")).toBeDefined(); // accepted, of what was decided
  expect(line.getByText("80%")).toBeDefined(); // gates
  expect(line.getByText("1.33")).toBeDefined(); // attempts per slice landed
  expect(line.getByText("65k")).toBeDefined(); // every input token sent
  expect(line.getByText("3/4")).toBeDefined(); // the counts behind the rate
});

it("a pairing with no history shows a dash, never a zero", async () => {
  // Rendering 0% for a model nobody has tried is how a good model gets retired.
  stub([
    row({
      group: "reviewer",
      attempts: 0,
      accepted: 0,
      rejected: 0,
      slices_accepted: 0,
      accepted_rate: null,
      rework: null,
      input_per_accepted: null,
      cache_hit_rate: null,
      yield_per_k: null,
      gate_pass_rate: null,
      cycle_time: null,
    }),
  ]);
  render(<Analytics tick={0} />);

  const line = within(await screen.findByRole("row", { name: /reviewer/ }));
  expect(line.queryByText("0%")).toBeNull();
  expect(line.getAllByText("-").length).toBeGreaterThan(3);
});

it("marks a seat whose context is almost never served from cache", async () => {
  // The demo's second question: which cold nodes are burning rate limit on prefix alone.
  const { container } = (() => {
    stub([
      row({ group: "cold", cache_read: 0, cache_write: 64_000, cache_hit_rate: 0 }),
      row({ group: "warm", cache_hit_rate: 0.94 }),
    ]);
    return render(<Analytics tick={0} />);
  })();

  await screen.findByText("cold");
  const marked = container.querySelectorAll('td[data-cold="true"]');
  expect(marked.length).toBe(1);
  expect(marked[0]?.textContent).toBe("0%");
});

it("cuts the same history four ways", async () => {
  // Two seats on one model is a fact about the model; one seat across two models is the
  // comparison worth making.
  const user = userEvent.setup();
  const calls = stub([row({})]);
  render(<Analytics tick={0} />);
  await screen.findByText("backend");

  await user.click(screen.getByText("model"));
  await waitFor(() => expect(calls.some((url) => url.includes("by=model"))).toBe(true));
});

it("is global, and never scoped by a selection somewhere else", async () => {
  // D18: "which pairing earns its seat" is not a question about one repository, and the
  // `project` grouping already answers the per-project version without a filter elsewhere
  // silently changing what the page means.
  const calls = stub([row({})]);
  render(<Analytics tick={0} />);
  await waitFor(() => expect(calls.length).toBeGreaterThan(0));
  expect(calls.every((url) => !url.includes("project="))).toBe(true);
});

it("says plainly when there is nothing to compare", async () => {
  stub([]);
  render(<Analytics tick={0} />);
  expect(await screen.findByText(/nothing to compare/)).toBeDefined();
});

it("says what is wrong rather than showing an empty table", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      json: async () => ({ error: "no database yet - run `ait init`" }),
    }),
  );
  render(<Analytics tick={0} />);
  expect(await screen.findByText(/ait init/)).toBeDefined();
});
