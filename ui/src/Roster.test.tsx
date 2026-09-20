import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Roster } from "./Roster";
import type { Seat } from "./api";

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

function stub(options: { seats?: Seat[]; available?: string[]; project?: string | null } = {}) {
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
      if (url.startsWith("/roster")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            project: options.project ?? null,
            team: null,
            seats: options.seats ?? [seat()],
            available: options.available ?? ["claude", "local"],
          }),
        });
      }
      return Promise.resolve({ ok: true, json: async () => ({}) });
    }),
  );
  return calls;
}

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

it("changing a provider sends it and tells the rest of the window", async () => {
  const user = userEvent.setup();
  const calls = stub();
  let told = 0;
  render(<Roster onChanged={() => (told += 1)} />);

  await user.selectOptions(await screen.findByLabelText("provider for backend"), "local");
  await waitFor(() => expect(calls.some((c) => c.url === "/roster/3")).toBe(true));
  expect(calls.find((c) => c.url === "/roster/3")?.body).toEqual({ provider: "local" });
  await waitFor(() => expect(told).toBeGreaterThan(0));
});

it("the configured provider stays in the picker even when unavailable", async () => {
  // Otherwise the picker silently shows something the seat is not set to, and the next
  // change would move it without anybody asking.
  stub({ seats: [seat({ provider: "zai" })], available: ["claude"] });
  render(<Roster onChanged={() => {}} />);

  const picker = await screen.findByLabelText("provider for backend");
  expect((picker as HTMLSelectElement).value).toBe("zai");
  expect(screen.getByText(/zai \(unavailable\)/)).toBeDefined();
});

it("with no provider available it says so rather than offering an empty picker", async () => {
  stub({ available: [] });
  render(<Roster onChanged={() => {}} />);
  expect(await screen.findByText(/No provider is available/)).toBeDefined();
});

it("defaults are shown when no project is chosen, and are not editable", async () => {
  // They are not rows yet. A picker that appears to change them would change nothing.
  stub({ seats: [seat({ id: -1 })], project: null });
  render(<Roster onChanged={() => {}} />);

  expect(await screen.findByText(/seats a new project gets/)).toBeDefined();
  expect((await screen.findByLabelText("provider for backend")).getAttribute("disabled")).not.toBeNull();
  expect(screen.queryByText("Disable")).toBeNull();
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

it("says when a change takes effect, because an edit that looks applied and is not is the confusing case", async () => {
  stub();
  render(<Roster onChanged={() => {}} />);
  expect(await screen.findByText(/take effect on the next run/)).toBeDefined();
});
