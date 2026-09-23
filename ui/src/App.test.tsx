import { render, screen, waitFor, within } from "@testing-library/react";
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
const READY = {
  version: "0.1.0",
  checks: [],
  severity: "fine",
  can_run: true,
  needs_setup: false,
};
const WIDGET = {
  id: 1,
  slug: "widget",
  name: "Widget",
  kind: "repo",
  status: "active",
  open_runs: 1,
};

/** What the overview reads. Entering a project lands there, so every test that opens
 *  one needs these whether or not it is about them. */
const CHECKOUT = {
  "/worktrees": [
    {
      name: "main",
      path: "/tmp/widget",
      status: "main",
      lease_holder: null,
      processes: [],
      branch: "main",
      main: true,
    },
    {
      name: "1",
      path: "/tmp/widget-task",
      status: "in-use",
      lease_holder: null,
      processes: [],
      branch: "feature/task",
      main: false,
    },
  ],
  "/map": {
    root: "widget",
    files: 0,
    unowned: 0,
    truncated: false,
    nodes: [],
    edges: [],
    zones: [],
  },
  "/roster": { project: "widget", team: "Widget team", seats: [], available: [] },
  "/models": { models: [], error: null },
  "/crew": [],
  "/analytics": [],
  "/board": { plan: null, slices: [], next_step: null },
};

/** A machine that is set up, with one project and one run in it. */
function working(over: Record<string, unknown> = {}) {
  stubApi({
    "/health": HEALTH,
    "/doctor": READY,
    "/projects": [WIDGET],
    "/today": [],
    ...CHECKOUT,
    "/runs": [
      {
        id: 7,
        project_id: 1,
        prompt: "add subtract",
        status: "running",
        trigger: "manual",
        created_at: "",
        started_at: null,
        ended_at: null,
      },
    ],
    ...over,
  });
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

it("opens on Today, because that is the question you have when you open the window", async () => {
  working();
  render(<App />);
  expect(await screen.findByText(/Nothing is waiting on you/)).toBeDefined();
  expect(screen.queryByText(/Let's get you set up/)).toBeNull();
});

it("the global views do not take a project", async () => {
  // D18. Every one of these used to be silently scoped by a selection elsewhere, which made
  // each answer a slightly different question depending on something off screen.
  working();
  render(<App />);

  const everything = within(await screen.findByLabelText("Everything"));
  for (const name of ["Today", "Analytics", "Schedule", "Projects", "Settings"]) {
    expect(everything.getByText(name)).toBeDefined();
  }
  // And the project-level ones are not offered until you are in a project.
  for (const name of ["Board", "Review", "Editor", "Terminal", "Source"]) {
    expect(everything.queryByText(name)).toBeNull();
  }
});

it("selecting a project enters it, and leaves the global views alone", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);

  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );

  // Its own navigation replaces the global list: two lists of views is two places to look
  // for the same thing.
  const area = within(await screen.findByLabelText("Widget"));
  expect(area.getByText("Work")).toBeDefined();
  expect(area.getByText("Board")).toBeDefined();
  expect(screen.queryByLabelText("Everything")).toBeNull();

  // And the way back out.
  await user.click(area.getByText("← Everything"));
  expect(await screen.findByLabelText("Everything")).toBeDefined();
});

it("shows task worktrees beneath their repository and scopes the whole workspace when selected", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);

  const nested = within(await screen.findByLabelText("Widget workspaces"));
  await user.click(await nested.findByText("feature/task"));
  await screen.findByText(/every path, and the seat whose zone claims it/i);

  const calls = vi.mocked(fetch).mock.calls.map(([url]) => String(url));
  for (const route of ["/api/map?", "/api/board?", "/api/crew?", "/api/runs?"]) {
    const call = calls.find((url) => url.startsWith(route) && url.includes("widget-task"));
    expect(call, `${route} was not scoped to the selected checkout`).toBeDefined();
  }
});

it("starting from a child checkout launches the team there and stays there", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);

  const nested = within(await screen.findByLabelText("Widget workspaces"));
  await user.click(await nested.findByText("feature/task"));
  await user.click(within(screen.getByLabelText("Widget")).getByText("Work"));
  await user.type(await screen.findByRole("textbox"), "build the whole feature");
  await user.click(screen.getByRole("button", { name: "Start" }));

  await waitFor(() => {
    const call = vi
      .mocked(fetch)
      .mock.calls.find(
        ([url, init]) => String(url) === "/api/runs" && init?.method === "POST",
      );
    expect(call).toBeDefined();
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({
      project: "widget",
      workspace: "/tmp/widget-task",
      prompt: "build the whole feature",
      approval_required: true,
    });
  });
  expect(nested.getByText("feature/task").closest("button")?.getAttribute("aria-current")).toBe(
    "true",
  );
});

it("entering a project starts on the overview, which says what the repository is", async () => {
  // What the checkout is comes before what you were last doing to it: the landing view
  // answers "whose is this, and how much of it can the team be given".
  const user = userEvent.setup();
  working();
  render(<App />);

  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );
  expect(await screen.findByText(/every path, and the seat whose zone claims it/i)).toBeDefined();
});

it("Work is one click away, and still lists the runs", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);

  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );
  await user.click(within(screen.getByLabelText("Widget")).getByText("Work"));
  expect(await screen.findByText("add subtract")).toBeDefined();
});

it("coming back to a project keeps where you were in it", async () => {
  const user = userEvent.setup();
  working({
    "/board": { plan: null, slices: [], next_step: "Run `aip new` in it." },
  });
  render(<App />);

  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );
  await user.click(within(screen.getByLabelText("Widget")).getByText("Board"));
  await screen.findByText(/aip new/);

  // Out and back in.
  await user.click(within(screen.getByLabelText("Widget")).getByText("← Everything"));
  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );
  expect(await screen.findByText(/aip new/)).toBeDefined();
});

it("following an item from Today enters the project it belongs to", async () => {
  // Today spans projects, so the id alone is not enough - guessing would open the wrong
  // workspace, and rendering the run in a second place would drift from the first.
  const user = userEvent.setup();
  working({
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
  expect(await screen.findByLabelText("Widget")).toBeDefined();
  expect(await screen.findByText("Run #7")).toBeDefined();
});

it("Escape closes the innermost surface first", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);

  await user.click(await screen.findByText("About"));
  expect(await screen.findByRole("dialog")).toBeDefined();

  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

it("the theme is remembered, and every colour follows it", async () => {
  // The switch lives in Settings now; this is about the attribute, which is the whole
  // mechanism - every surface resolves a token rather than naming a colour.
  const user = userEvent.setup();
  working({
    "/settings": {
      profile_path: "/x",
      providers: [],
      fallback: ["claude", "openai", "zai", "local"],
      context: [],
    },
  });

  const { unmount } = render(<App />);
  await user.click(await screen.findByText("Settings"));
  await user.click(await screen.findByText("light"));
  expect(document.documentElement.dataset.theme).toBe("light");

  unmount();
  render(<App />);
  await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
});

it("re-reads when the server says something changed", async () => {
  const user = userEvent.setup();
  working();
  render(<App />);
  await user.click(
    within(await screen.findByRole("navigation", { name: "Projects" })).getByRole("button", {
      name: /Widget/,
    }),
  );
  // On Work, because the run list is what this is about - the overview is the landing
  // view but it is not where runs are read.
  await user.click(within(screen.getByLabelText("Widget")).getByText("Work"));
  await screen.findByText("add subtract");

  // A run started from the CLI, in another process entirely.
  working({
    "/runs": [
      {
        id: 7,
        project_id: 1,
        prompt: "add subtract",
        status: "done",
        trigger: "manual",
        created_at: "",
        started_at: null,
        ended_at: null,
      },
      {
        id: 8,
        project_id: 1,
        prompt: "second",
        status: "running",
        trigger: "manual",
        created_at: "",
        started_at: null,
        ended_at: null,
      },
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
  // More than one surface says so, which is right: the shell cannot list projects and Today
  // cannot list anything either, and each reports what it found.
  expect((await screen.findAllByText("unauthorized")).length).toBeGreaterThan(0);
});
