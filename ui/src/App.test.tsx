import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import App from "./App";

/** A fetch that answers each API path from a fixture. */
function stubApi(routes: Record<string, unknown>) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = String(input).replace(/^\/api/, "").replace(/\?.*$/, "");
      const body = routes[path];
      if (body === undefined) {
        return Promise.resolve({ ok: false, status: 404, json: async () => ({ error: path }) });
      }
      return Promise.resolve({ ok: true, json: async () => body });
    }),
  );
}

/** EventSource does not exist in jsdom, and the shell opens one on mount. */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  onmessage: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor() {
    FakeEventSource.last = this;
  }
  close() {
    this.closed = true;
  }
}

beforeEach(() => {
  vi.stubGlobal("EventSource", FakeEventSource);
  FakeEventSource.last = null;
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute("data-theme");
  localStorage.clear();
});

const HEALTH = { version: "0.1.0", bundle_embedded: true, bundle_files: 3 };

/** The window opens on Today, so a test about the Console has to go there first. */
async function openConsole(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByText("Console"));
}

it("opens on setup when this machine has not been set up", async () => {
  // The confusing first impression this replaces: a fresh install opened on an empty Today
  // with no indication that nothing could run or what to do about it.
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [],
    "/today": [],
    "/doctor": {
      version: "0.1.0",
      checks: [
        {
          id: "database",
          label: "Database",
          severity: "blocking",
          detail: "not created yet",
          fix: { by: "itself", action: "create_database", describe: "Create it" },
        },
      ],
      severity: "blocking",
      can_run: false,
      needs_setup: true,
    },
    "/settings": { profile_path: "/x", providers: [], fallback: [], context: [] },
  });
  render(<App />);
  expect(await screen.findByText(/Let's get you set up/)).toBeDefined();
});

it("opens on Today when the machine is ready", async () => {
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [],
    "/today": [],
    "/doctor": {
      version: "0.1.0",
      checks: [],
      severity: "fine",
      can_run: true,
      needs_setup: false,
    },
  });
  render(<App />);
  expect(await screen.findByText(/Nothing is waiting on you/)).toBeDefined();
  expect(screen.queryByText(/Let's get you set up/)).toBeNull();
});

it("opens on Today, because that is the question you have when you open the window", async () => {
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [],
    "/today": [
      {
        urgency: "blocking",
        kind: "approval",
        title: "may I commit?",
        detail: "run #7 is waiting on you",
        project: "widget",
        run_id: 7,
        since: "2026-09-18T08:00:00Z",
      },
    ],
  });
  render(<App />);
  expect(await screen.findByText("Do this first")).toBeDefined();
  expect(await screen.findByText("may I commit?")).toBeDefined();
});

it("following an item from Today opens that run in the Console", async () => {
  // Today answers "what now"; the run itself is the Console's job. A third surface that
  // renders runs slightly differently is how two views start disagreeing.
  const user = userEvent.setup();
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [],
    "/today": [
      {
        urgency: "failed",
        kind: "node",
        title: "backend failed on S1",
        detail: null,
        project: "widget",
        run_id: 7,
        since: null,
      },
    ],
    "/runs/7": {
      id: 7,
      project_id: 1,
      prompt: "add subtract",
      status: "failed",
      trigger: "manual",
      created_at: "",
      started_at: null,
      ended_at: null,
      nodes: [],
      usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 },
    },
    "/runs/7/events": [],
    "/runs/7/approvals": [],
  });

  render(<App />);
  await user.click(await screen.findByText("backend failed on S1"));
  expect(await screen.findByText("Run #7")).toBeDefined();
});

it("shows the runs the database has, and what each one is doing", async () => {
  stubApi({
    "/health": HEALTH,
    "/projects": [{ id: 1, slug: "widget", name: "Widget", kind: "repo", status: "active", open_runs: 1 }],
    "/runs": [{ id: 7, project_id: 1, prompt: "add subtract", status: "running", trigger: "manual", created_at: "", started_at: null, ended_at: null }],
  });

  const user = userEvent.setup();
  render(<App />);
  await openConsole(user);

  expect(await screen.findByText("Widget")).toBeDefined();
  expect(await screen.findByText("add subtract")).toBeDefined();
  // The status is a word, not only a colour - a dot alone is unreadable to anyone who
  // cannot tell these two apart.
  expect(await screen.findByText("running")).toBeDefined();
});

it("says what to do when there are no runs yet", async () => {
  const user = userEvent.setup();
  stubApi({ "/health": HEALTH, "/projects": [], "/runs": [] });
  render(<App />);
  await openConsole(user);
  expect(await screen.findByText(/No runs yet/)).toBeDefined();
  expect(await screen.findByText(/ait run/)).toBeDefined();
});

it("opens a run in the dock and closes it with Escape", async () => {
  const user = userEvent.setup();
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [{ id: 7, project_id: 1, prompt: "add subtract", status: "done", trigger: "manual", created_at: "", started_at: null, ended_at: null }],
    "/runs/7": {
      id: 7,
      project_id: 1,
      prompt: "add subtract",
      status: "done",
      trigger: "manual",
      created_at: "",
      started_at: null,
      ended_at: null,
      nodes: [{ id: 1, role: "backend", provider: "claude", model: "sonnet", status: "done", attempt: 1, slice_key: "S1", branch: "ai-team/s1", blocked_reason: null }],
      usage: { tokens_in: 1, tokens_out: 2, cache_read: 0, cache_write: 0 },
    },
    "/runs/7/events": [{ id: 1, kind: "note", actor: null, summary: "did the thing", at: "" }],
    "/runs/7/approvals": [],
  });

  render(<App />);
  await openConsole(user);
  await user.click(await screen.findByText("add subtract"));

  expect(await screen.findByText("Run #7")).toBeDefined();
  expect(await screen.findByText("ai-team/s1")).toBeDefined();
  expect(await screen.findByText("did the thing")).toBeDefined();

  // Esc dismisses the topmost surface. With only the dock open, that is the dock.
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByText("Run #7")).toBeNull());
});

it("Escape closes the innermost surface first", async () => {
  const user = userEvent.setup();
  stubApi({ "/health": HEALTH, "/projects": [], "/runs": [] });
  render(<App />);

  await user.click(await screen.findByText("About"));
  expect(await screen.findByRole("dialog")).toBeDefined();

  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

it("switching theme repaints the document, and the choice survives a reload", async () => {
  const user = userEvent.setup();
  stubApi({ "/health": HEALTH, "/projects": [], "/runs": [] });

  const { unmount } = render(<App />);
  await user.click(await screen.findByText("light"));
  expect(document.documentElement.dataset.theme).toBe("light");

  // Every colour comes from the token layer, so this attribute is the whole switch.
  unmount();
  render(<App />);
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
});

it("re-reads when the server says something changed", async () => {
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [{ id: 7, project_id: 1, prompt: "first", status: "running", trigger: "manual", created_at: "", started_at: null, ended_at: null }],
  });
  const user = userEvent.setup();
  render(<App />);
  await openConsole(user);
  await screen.findByText("first");

  // A run started from the CLI, in another process entirely.
  stubApi({
    "/health": HEALTH,
    "/projects": [],
    "/runs": [
      { id: 7, project_id: 1, prompt: "first", status: "done", trigger: "manual", created_at: "", started_at: null, ended_at: null },
      { id: 8, project_id: 1, prompt: "second", status: "running", trigger: "manual", created_at: "", started_at: null, ended_at: null },
    ],
  });
  FakeEventSource.last?.onmessage?.();

  expect(await screen.findByText("second")).toBeDefined();
});

it("says so when the server cannot be reached", async () => {
  // The failure mode worth rendering: the page loaded from the bundle, so the binary is
  // fine, but the API refused it - which is almost always a stale token after a restart.
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
      json: async () => ({ error: "unauthorized" }),
    }),
  );

  render(<App />);
  expect(await screen.findByText("unauthorized")).toBeDefined();
});
