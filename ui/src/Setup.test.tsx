import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { HealthBanner, Setup } from "./Setup";
import type { Check } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function check(over: Partial<Check> & Pick<Check, "id">): Check {
  return {
    label: over.id,
    severity: "blocking",
    detail: "",
    fix: { by: "none" },
    ...over,
  } as Check;
}

/** Answers /doctor and /settings, and records writes. Fixes flip the report. */
function stub(options: {
  checks?: Check[];
  providers?: unknown[];
  canRun?: boolean;
  needsSetup?: boolean;
  after?: Check[];
} = {}) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  let fixed = false;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      const method = init?.method ?? "GET";
      calls.push({
        url,
        method,
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });

      if (url === "/doctor" && method === "GET") {
        const checks = fixed && options.after !== undefined ? options.after : options.checks ?? [];
        return Promise.resolve({
          ok: true,
          json: async () => ({
            version: "0.1.0",
            checks,
            severity: checks.some((c) => c.severity === "blocking") ? "blocking" : "fine",
            can_run: options.canRun ?? false,
            needs_setup: options.needsSetup ?? true,
          }),
        });
      }
      if (url === "/settings") {
        return Promise.resolve({
          ok: true,
          json: async () => ({
            profile_path: "/home/me/.config/ai-team/machine.toml",
            providers: options.providers ?? [],
            fallback: ["claude", "openai", "zai", "local"],
            context: [],
          }),
        });
      }
      fixed = true;
      return Promise.resolve({ ok: true, json: async () => ({ done: "ok" }) });
    }),
  );
  return calls;
}

const DB = check({
  id: "database",
  label: "Database",
  detail: "not created yet",
  fix: { by: "itself", action: "create_database", describe: "Create it and seed a team" },
});

it("offers to create what ai-team owns, one thing at a time", async () => {
  // "Fix everything" is a button people press without reading.
  const user = userEvent.setup();
  const calls = stub({ checks: [DB], after: [] });
  render(<Setup onReady={() => {}} />);

  expect(await screen.findByText("Create it and seed a team")).toBeDefined();
  await user.click(screen.getByText("Do it"));

  await waitFor(() => expect(calls.some((c) => c.url === "/doctor/fix")).toBe(true));
  expect(calls.find((c) => c.url === "/doctor/fix")?.body).toEqual({
    action: "create_database",
  });
});

it("never offers to run an install, only to copy it", async () => {
  // D17. `curl | sh` from a GUI button is a decision that is not ai-team's to take.
  stub({
    checks: [
      check({
        id: "aip",
        label: "ai-planner",
        detail: "not installed",
        fix: { by: "command", run: "curl -fsSL https://example/install.sh | sh", why: "it plans" },
      }),
    ],
  });
  render(<Setup onReady={() => {}} />);

  expect(await screen.findByText("curl -fsSL https://example/install.sh | sh")).toBeDefined();
  expect(screen.getByText("Copy")).toBeDefined();
  expect(screen.queryByText("Do it")).toBeNull();
  expect(screen.getByText(/will not install software/)).toBeDefined();
});

it("marks whether a missing tool is needed or optional", async () => {
  // A blocking ai-planner and an optional file-sql are different problems, and treating
  // them the same makes the list one somebody skims.
  stub({
    checks: [
      check({
        id: "aip",
        label: "ai-planner",
        fix: { by: "command", run: "a", why: "needed for planning" },
      }),
      check({
        id: "file_sql",
        label: "file-sql",
        severity: "degraded",
        fix: { by: "command", run: "b", why: "search in the editor" },
      }),
    ],
  });
  render(<Setup onReady={() => {}} />);

  expect(await screen.findByText("needed")).toBeDefined();
  expect(screen.getByText("optional")).toBeDefined();
});

it("asks for a provider, and says which are signed in", async () => {
  stub({
    checks: [check({ id: "providers", label: "Model providers" })],
    providers: [
      {
        provider: "claude",
        label: "claude",
        allowed: true,
        reachable: false,
        detail: "",
        how: "through the Claude Code CLI",
      },
    ],
  });
  render(<Setup onReady={() => {}} />);

  expect(await screen.findByText(/Pick an account/)).toBeDefined();
  // Ticked and not signed in is the commonest mistake, and it fails minutes later.
  expect(screen.getByText("not signed in")).toBeDefined();
});

it("does not ask for a repository until there is something to think with", async () => {
  // Adding a repo on a machine with no provider produces a project that cannot run, which
  // is a worse first experience than being asked in order.
  stub({
    checks: [
      check({ id: "providers", label: "Model providers" }),
      check({ id: "projects", label: "Projects" }),
    ],
  });
  render(<Setup onReady={() => {}} />);

  await screen.findByText(/Pick an account/);
  expect(screen.queryByText(/What are we working on/)).toBeNull();
});

it("asks for a repository once a provider is ready", async () => {
  const user = userEvent.setup();
  const calls = stub({ checks: [check({ id: "projects", label: "Projects" })] });
  render(<Setup onReady={() => {}} />);

  await user.type(await screen.findByLabelText("path"), "/Users/me/Developer/widget");
  await user.click(screen.getByText("Add"));

  await waitFor(() => expect(calls.some((c) => c.url === "/projects")).toBe(true));
  expect(calls.find((c) => c.url === "/projects" && c.method === "POST")?.body).toMatchObject({
    path: "/Users/me/Developer/widget",
  });
});

it("only congratulates a machine that can actually run something", async () => {
  stub({
    checks: [],
    canRun: false,
    providers: [{ provider: "claude", label: "claude", allowed: true, reachable: false, detail: "", how: "" }],
  });
  render(<Setup onReady={() => {}} />);
  await waitFor(() => expect(screen.queryByText(/That's it/)).toBeNull());
});

it("says it is done when there is nothing left", async () => {
  const user = userEvent.setup();
  stub({
    checks: [],
    canRun: true,
    needsSetup: false,
    providers: [
      { provider: "claude", label: "claude", allowed: true, reachable: true, detail: "", how: "" },
    ],
  });
  const opened: number[] = [];
  render(<Setup onReady={() => opened.push(1)} />);

  expect(await screen.findByText(/That's it/)).toBeDefined();
  expect(screen.getByText(/claude is ready/)).toBeDefined();
  await user.click(screen.getByText("Open ai-team"));
  expect(opened.length).toBeGreaterThan(0);
});

it("the health banner shows only what is blocking", async () => {
  // A banner that appears for a missing file-sql is one people learn to ignore - and then
  // it is not there when the database has gone.
  const { container, unmount } = render(<>{null}</>);
  unmount();

  stub({ checks: [check({ id: "file_sql", severity: "degraded" })] });
  const degraded = render(<HealthBanner tick={0} onOpen={() => {}} />);
  await waitFor(() => expect(degraded.container.querySelector(".health")).toBeNull());
  degraded.unmount();

  vi.unstubAllGlobals();
  stub({ checks: [DB] });
  const blocking = render(<HealthBanner tick={0} onOpen={() => {}} />);
  await waitFor(() => expect(blocking.container.querySelector(".health")).not.toBeNull());
  expect(blocking.getByText("Database")).toBeDefined();
  expect(container).toBeDefined();
});

it("the banner opens setup when clicked", async () => {
  const user = userEvent.setup();
  stub({ checks: [DB] });
  const opened: number[] = [];
  render(<HealthBanner tick={0} onOpen={() => opened.push(1)} />);

  await user.click(await screen.findByText("Database"));
  expect(opened.length).toBe(1);
});

it("a report it cannot fetch is silence, not a broken banner", async () => {
  vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("down")));
  const { container } = render(<HealthBanner tick={0} onOpen={() => {}} />);
  await waitFor(() => expect(container.querySelector(".health")).toBeNull());
  expect(container.querySelector(".error")).toBeNull();
});
