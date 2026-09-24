import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Roster } from "./Roster";
import type { ModelChoice, Seat } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function seat(over: Partial<Seat> = {}): Seat {
  return {
    id: 3,
    role: "backend",
    name: "Backend",
    purpose: "Builds the server",
    provider: "claude",
    model: "sonnet",
    effective_provider: "claude",
    effective_model: "sonnet",
    fallback_reason: null,
    reasoning: "medium",
    zone: "src/**",
    read_only: false,
    enabled: true,
    ...over,
  };
}

const choices: ModelChoice[] = [
  {
    provider: "claude",
    runtime_provider: "claude-subscription",
    model: "claude-opus-5",
    context: "1M",
    max_output: "128K",
    thinking: true,
    images: true,
  },
  {
    provider: "local",
    runtime_provider: "llama.cpp",
    model: "Qwen3-Coder-Next",
    context: "128K",
    max_output: "32K",
    thinking: true,
    images: false,
  },
];

function stub(options: {
  seats?: Seat[];
  available?: string[];
  project?: string | null;
  models?: ModelChoice[];
  catalogError?: string | null;
  delivery?: { push: "manual" | "ask" | "auto"; pr: "manual" | "ask" | "auto"; merge: "manual" | "ask" | "auto" };
} = {}) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (url === "/projects") {
        return Promise.resolve({
          ok: true,
          json: async () => [
            { id: 1, slug: "widget", name: "Widget", kind: "repo", status: "active", open_runs: 0 },
          ],
        });
      }
      if (url === "/roster/models/reseat") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            moved: [
              { role: "orchestrator", to: "openai/gpt-6-astra" },
              { role: "planner", to: "claude/claude-opus-5" },
            ],
            left: [{ role: "reviewer", why: "Pi has no llama.cpp/auto on this machine" }],
          }),
        });
      }
      if (url.startsWith("/roster")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            project: options.project ?? null,
            team: null,
            delivery: options.delivery ?? { push: "ask", pr: "ask", merge: "ask" },
            seats: options.seats ?? [seat()],
            available: options.available ?? ["claude", "local"],
          }),
        });
      }
      if (url === "/models") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            models: options.models ?? choices,
            error: options.catalogError ?? null,
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => ({}) });
    }),
  );
  return calls;
}

it("edits each remote delivery boundary independently", async () => {
  const user = userEvent.setup();
  const calls = stub({ project: "widget" });
  render(<Roster project="widget" onChanged={() => {}} />);

  await user.selectOptions(await screen.findByLabelText("push policy"), "auto");

  await waitFor(() =>
    expect(calls).toContainEqual({
      url: "/roster/delivery",
      method: "POST",
      body: { project: "widget", push: "auto", pr: "ask", merge: "ask" },
    }),
  );
});

it("shows what a seat will actually run as when that differs from its setting", async () => {
  // D13: a seat set to a provider the machine denies falls back through the ranking. A
  // roster that hides that lies about which account the work lands on.
  stub({
    seats: [
      seat({
        provider: "openai",
        model: "gpt-5.6-luna-fast",
        effective_provider: "claude",
        effective_model: "sonnet",
        fallback_reason: "openai is denied on this machine",
      }),
    ],
  });
  render(<Roster onChanged={() => {}} />);

  expect(await screen.findByText(/Runs as claude\/sonnet/)).toBeDefined();
  expect(screen.getByText(/openai is denied on this machine/)).toBeDefined();
});

it("says nothing extra when the seat runs as configured", async () => {
  stub();
  render(<Roster onChanged={() => {}} />);
  await screen.findByText("Backend");
  expect(screen.queryByText(/Runs as/)).toBeNull();
});

it("changing a model sends its exact Pi provider and id", async () => {
  const user = userEvent.setup();
  const calls = stub();
  let told = 0;
  render(<Roster onChanged={() => (told += 1)} />);

  await user.selectOptions(
    await screen.findByLabelText("model for backend"),
    JSON.stringify(["local", "Qwen3-Coder-Next"]),
  );
  await waitFor(() => expect(calls.some((c) => c.url === "/roster/3")).toBe(true));
  expect(calls.find((c) => c.url === "/roster/3")?.body).toEqual({
    provider: "local",
    model: "Qwen3-Coder-Next",
  });
  await waitFor(() => expect(told).toBeGreaterThan(0));
});

it("lists exact usable Pi model ids and their limits", async () => {
  stub();
  render(<Roster onChanged={() => {}} />);

  await screen.findByLabelText("model for backend");
  expect(screen.getByText("claude-opus-5 · 1M context · 128K max")).toBeDefined();
  expect(screen.getByText("Qwen3-Coder-Next · 128K context · 32K max")).toBeDefined();
});

it("the configured model stays in the picker even when unavailable", async () => {
  // Otherwise the picker silently shows something the seat is not set to, and the next
  // change would move it without anybody asking.
  stub({ seats: [seat({ provider: "zai", model: "glm-old" })] });
  render(<Roster onChanged={() => {}} />);

  const picker = await screen.findByLabelText("model for backend");
  expect((picker as HTMLSelectElement).value).toBe(JSON.stringify(["zai", "glm-old"]));
  expect(screen.getByText(/zai-coding-plan\/glm-old \(unavailable\)/)).toBeDefined();
});

it("with no usable model it says so rather than offering an empty picker", async () => {
  stub({ models: [] });
  render(<Roster onChanged={() => {}} />);
  expect(await screen.findByText(/Pi reports no usable model/)).toBeDefined();
});

it("defaults are shown when no project is chosen, and are not editable", async () => {
  // They are not rows yet. A picker that appears to change them would change nothing.
  stub({ seats: [seat({ id: -1 })], project: null });
  render(<Roster onChanged={() => {}} />);

  expect(await screen.findByText(/seats a new project gets/)).toBeDefined();
  expect((await screen.findByLabelText("model for backend")).getAttribute("disabled")).not.toBeNull();
  expect(screen.queryByText("Disable")).toBeNull();
});

it("edits a maker's ownership patterns", async () => {
  const user = userEvent.setup();
  const calls = stub({ project: "widget" });
  render(<Roster project="widget" onChanged={() => {}} />);

  const ownership = await screen.findByLabelText("ownership for backend");
  await user.clear(ownership);
  await user.type(ownership, "server/**\nconfig/**");
  await user.click(screen.getByText("Save ownership"));

  await waitFor(() =>
    expect(
      calls.some(
        (call) =>
          call.url === "/roster/3" &&
          JSON.stringify(call.body) === JSON.stringify({ zone: "server/**\nconfig/**" }),
      ),
    ).toBe(true),
  );
});

it("only re-detects repository ownership after explicit confirmation", async () => {
  const user = userEvent.setup();
  const calls = stub({ project: "widget" });
  render(<Roster project="widget" onChanged={() => {}} />);

  await user.click(await screen.findByText("Detect ownership"));
  expect(calls.some((call) => call.url === "/roster/ownership/detect")).toBe(false);
  await user.click(screen.getByText("Replace ownership"));
  await waitFor(() =>
    expect(
      calls.some(
        (call) =>
          call.url === "/roster/ownership/detect" &&
          JSON.stringify(call.body) === JSON.stringify({ project: "widget" }),
      ),
    ).toBe(true),
  );
});

it("keeps existing model choices until an explicit role-default reset is confirmed", async () => {
  const user = userEvent.setup();
  const calls = stub({ project: "widget" });
  render(<Roster project="widget" onChanged={() => {}} />);

  await user.click(await screen.findByText("Reset role models"));
  expect(calls.some((call) => call.url === "/roster/models/reset")).toBe(false);
  await user.click(screen.getByText("Replace models"));
  await waitFor(() =>
    expect(
      calls.some(
        (call) =>
          call.url === "/roster/models/reset" &&
          JSON.stringify(call.body) === JSON.stringify({ project: "widget" }),
      ),
    ).toBe(true),
  );
});

it("a seat Pi cannot run on this machine says so, and one it can says nothing", async () => {
  // Seeded while only the local gateway was allowed: the seat still points at it, and the
  // one hint used to be "(unavailable)" in a select box.
  stub({
    project: "widget",
    seats: [
      seat({
        id: 1,
        role: "orchestrator",
        provider: "local",
        model: "auto",
        effective_provider: "local",
        effective_model: "auto",
      }),
      seat({ id: 2, role: "planner", provider: "claude", model: "claude-opus-5",
        effective_provider: "claude", effective_model: "claude-opus-5" }),
      seat({ id: 3, role: "reviewer", provider: "local", model: "auto",
        effective_provider: "local", effective_model: "auto", enabled: false }),
    ],
  });
  render(<Roster project="widget" onChanged={() => {}} />);

  const warnings = await screen.findAllByText(/Cannot run on this machine/);
  // Only the orchestrator: the planner runs, and a seat switched off runs nowhere.
  expect(warnings).toHaveLength(1);
  expect(warnings[0]?.textContent).toBe(
    "Cannot run on this machine: Pi has no llama.cpp/auto, so its turns fail before a model is reached.",
  );
});

it("moves only the seats that cannot run here, once asked, and says what it did", async () => {
  const user = userEvent.setup();
  const stranded = (id: number, role: string) =>
    seat({ id, role, provider: "local", model: "auto", effective_provider: "local",
      effective_model: "auto" });
  const calls = stub({
    project: "widget",
    seats: [stranded(1, "orchestrator"), stranded(2, "planner")],
  });
  const changed: number[] = [];
  render(<Roster project="widget" onChanged={() => changed.push(1)} />);

  await user.click(await screen.findByText("Move 2 seats that cannot run here"));
  expect(calls.some((call) => call.url === "/roster/models/reseat")).toBe(false);
  await user.click(screen.getByText("Move them"));

  await waitFor(() =>
    expect(calls).toContainEqual({
      url: "/roster/models/reseat",
      method: "POST",
      body: { project: "widget" },
    }),
  );
  const result = await screen.findByRole("status");
  // One seat a line, on the project's own page: no project name, no run-on sentence.
  expect([...result.querySelectorAll("li")].map((item) => item.textContent)).toEqual([
    "orchestrator → openai/gpt-6-astra",
    "planner → claude/claude-opus-5",
    "reviewer stays: Pi has no llama.cpp/auto on this machine",
  ]);
  expect(changed.length).toBe(1);
});

it("offers no move when every seat can run", async () => {
  stub({
    project: "widget",
    seats: [seat({ model: "claude-opus-5", effective_model: "claude-opus-5" })],
  });
  render(<Roster project="widget" onChanged={() => {}} />);

  await screen.findByText("Reset role models");
  expect(screen.queryByText(/seats? that cannot run here/)).toBeNull();
  expect(screen.queryByText(/Cannot run on this machine/)).toBeNull();
});

it("a seat that owns nothing says so rather than showing an empty field", async () => {
  stub({ seats: [seat({ zone: "" })] });
  render(<Roster onChanged={() => {}} />);
  expect(await screen.findByText("nothing yet")).toBeDefined();
});

it("a read-only seat owning nothing is not a problem to report", async () => {
  // The verifier and the reviewer never edit, so an empty zone is correct for them.
  stub({ seats: [seat({ role: "verifier", read_only: true, zone: "" })] });
  render(<Roster onChanged={() => {}} />);
  expect(await screen.findByText(/does not edit/)).toBeDefined();
});

it("a disabled seat is marked and can be turned back on", async () => {
  const user = userEvent.setup();
  const calls = stub({ seats: [seat({ enabled: false })] });
  render(<Roster onChanged={() => {}} />);

  expect(await screen.findByText("off")).toBeDefined();
  await user.click(screen.getByText("Enable"));
  await waitFor(() => expect(calls.some((c) => c.url === "/roster/3")).toBe(true));
  expect(calls.find((c) => c.url === "/roster/3")?.body).toEqual({ enabled: true });
});

it("a seat's role sits beside its name, and its badges together at the end", async () => {
  // The header row spaces its children apart, so name, role and badges were each pushed
  // to a different place - the role floating mid-card, wherever the name's length left it.
  stub({ seats: [seat({ enabled: false })] });
  render(<Roster onChanged={() => {}} />);

  const who = (await screen.findByText("backend")).parentElement;
  const badges = screen.getByText("off").parentElement;
  expect(screen.getByText("Backend").parentElement).toBe(who);
  expect(screen.getByText("writes").parentElement).toBe(badges);
  // Two groups in one row, so the row's spacing falls between them and nowhere else.
  expect(badges).not.toBe(who);
  expect(badges?.parentElement).toBe(who?.parentElement);
  expect(who?.parentElement?.children).toHaveLength(2);
});

it("a seat's access is not dressed as a run state, which breathes as if it were working", async () => {
  // "writes" wore the running status, and a running status pulses - so every maker seat on
  // the Team page looked busy while nothing ran.
  stub({ seats: [seat(), seat({ id: 4, role: "verifier", name: "Verifier", read_only: true })] });
  render(<Roster onChanged={() => {}} />);

  for (const access of ["writes", "reads"]) {
    const badge = await screen.findByText(access);
    expect(badge.getAttribute("data-status")).toBeNull();
    expect(badge.getAttribute("data-access")).toBe(access);
  }
});

it("says when a change takes effect, because an edit that looks applied and is not is the confusing case", async () => {
  stub();
  render(<Roster onChanged={() => {}} />);
  // A seat is read as each turn starts, so a run already going takes an edit at that seat's
  // next turn. Saying "the next run" promised a run in progress would not change.
  expect(
    await screen.findByText(
      "Changes take effect at each seat's next turn, including in a run already going.",
    ),
  ).toBeDefined();
});
