import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { Overview } from "./Overview";
import type { Board, BoardSlice, Member, Project, RepoMap, Roster, Run } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

const WIDGET: Project = {
  id: 1,
  slug: "widget",
  name: "Widget",
  kind: "repo",
  status: "active",
  open_runs: 0,
};

function map(over: Partial<RepoMap> = {}): RepoMap {
  return {
    root: "widget",
    files: 10,
    unowned: 2,
    truncated: false,
    nodes: [
      { path: "", name: "widget", dir: true, depth: 0, weight: 10, owner: "backend" },
      { path: "src", name: "src", dir: true, depth: 1, weight: 8, owner: "backend" },
      { path: "src/lib.rs", name: "lib.rs", dir: false, depth: 2, weight: 1, owner: "backend" },
      { path: "README.md", name: "README.md", dir: false, depth: 1, weight: 1, owner: null },
    ],
    edges: [
      { from: 0, to: 1 },
      { from: 1, to: 2 },
      { from: 0, to: 3 },
    ],
    zones: [
      { role: "backend", name: "Backend", zone: "src/**", owns: 8 },
      { role: "mobile", name: "Mobile", zone: "ios/**", owns: 0 },
    ],
    ...over,
  };
}

function seat(role: string) {
  return {
    id: role === "backend" ? 1 : 2,
    role,
    name: role === "backend" ? "Backend" : "Mobile",
    purpose: "",
    provider: "local",
    model: "auto",
    effective_provider: "local",
    effective_model: "auto",
    fallback_reason: null,
    reasoning: "none",
    zone: role === "backend" ? "src/**" : "ios/**",
    read_only: false,
    enabled: true,
  };
}

function member(over: Partial<Member> = {}): Member {
  return {
    agent_id: 1,
    role: "backend",
    name: "Backend",
    provider: "local",
    model: "auto",
    read_only: false,
    zone: "src/**",
    doing: "idle",
    node_run_id: null,
    run_id: null,
    slice_key: null,
    branch: null,
    attempt: 0,
    blocked_reason: null,
    last_said: null,
    reachable: false,
    turns: 0,
    tokens_in: 0,
    tokens_out: 0,
    ...over,
  };
}

function slice(key: string, status: string, touches: string[]): BoardSlice {
  return {
    key,
    title: key,
    status,
    ord: 1,
    scope_md: null,
    demo_md: null,
    claimed_by: null,
    owner: null,
    touches,
  };
}

function board(over: Partial<Board> = {}): Board {
  return { plan: null, slices: [], next_step: null, ...over };
}

function stub(parts: {
  map?: RepoMap;
  board?: Board;
  roster?: Roster;
  crew?: Member[];
  runs?: Run[];
  gates?: unknown[];
}) {
  const routes: Record<string, unknown> = {
    "/map": parts.map ?? map(),
    "/board": parts.board ?? board(),
    "/roster": parts.roster ?? {
      project: "widget",
      team: "Widget team",
      seats: [seat("backend"), seat("mobile")],
      available: [],
    },
    "/crew": parts.crew ?? [],
    "/runs": parts.runs ?? [],
    "/analytics": parts.gates ?? [],
  };
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

function sheet() {
  return render(<Overview project={WIDGET} tick={0} onGo={() => {}} />);
}

/** A run in flight, with one seat mid-turn on a slice that names `src/**`. */
function busyParts() {
  return {
    crew: [member({ doing: "working", slice_key: "M1-S1", turns: 3, tokens_in: 900 })],
    board: board({
      plan: { plan: "widget", title: "Ship it", status: "active", slice: null },
      slices: [slice("M1-S1", "active", ["src/**"])],
    }),
    runs: [
      {
        id: 7,
        project_id: 1,
        prompt: "add subtract",
        status: "running",
        trigger: "manual",
        created_at: "",
        started_at: new Date().toISOString(),
        ended_at: null,
      } satisfies Run,
    ],
  };
}

it("leads with what the checkout is, not with what has been done to it", async () => {
  // The landing view of a repository nobody has run anything against still has something
  // true to say: how big it is, and how much of it the team can actually be given.
  stub({});
  sheet();

  expect(await screen.findByText("Widget")).toBeTruthy();
  expect(await screen.findByText("10")).toBeTruthy();
  // 8 of 10 files owned.
  expect(screen.getByText("80%")).toBeTruthy();
  expect(screen.getByText("nothing running")).toBeTruthy();
});

it("says there is no plan rather than drawing an empty board", async () => {
  stub({});
  sheet();
  expect(await screen.findByText(/No plan on this checkout/)).toBeTruthy();
});

it("names what nobody owns, because that is work a run reports undone", async () => {
  // The number the roster's text could never give: a path outside every zone is not
  // given to somebody, it is left (D14).
  stub({});
  sheet();
  expect(await screen.findByText(/2 paths belong to no zone/)).toBeTruthy();
});

it("says so when a seat's zone claims nothing in this repository", async () => {
  // A seat configured for a directory this checkout does not have will never be given
  // work, and nothing else in the window says that out loud.
  stub({});
  sheet();

  await screen.findByText("Mobile");
  expect(screen.getAllByText("owns nothing").length).toBe(1);
});

it("reports a gate pass rate only once a gate has run", async () => {
  stub({});
  sheet();
  expect(await screen.findByText(/No gate has run here yet/)).toBeTruthy();
});

it("is completely still while nothing is running", async () => {
  // The half of the design that makes the other half mean anything. A page that animates
  // whatever the state cannot be glanced at to find out whether anything is happening.
  stub({});
  sheet();

  await screen.findByText("nothing running");
  expect(document.querySelector('.overview[data-busy="false"]')).toBeTruthy();
  expect(document.querySelector('[data-state="hot"]')).toBeNull();
  expect(document.querySelector(".map__flow")).toBeNull();
});

it("lights the paths a live slice touches, and the route to them", async () => {
  // The join the whole picture rests on: a seat is mid-turn, its slice says it touches
  // `src/**`, and the map knows where those paths are - so they glow and the lines that
  // reach them carry a flow.
  stub(busyParts());
  sheet();

  await waitFor(() => expect(document.querySelector('.overview[data-busy="true"]')).toBeTruthy());
  // `src/**` claims what is *under* `src`, not the directory itself - the same answer
  // dispatch gives - so `src/lib.rs` is hot and nothing else is. The root never lights,
  // or the picture would say the whole repository is being worked on.
  expect(document.querySelectorAll('.map__node[data-state="hot"]').length).toBe(1);
  // The route still runs root → src → lib.rs, which is the line the work travels down.
  expect(document.querySelectorAll(".map__flow").length).toBe(2);
  expect(document.querySelector('.map[data-busy="true"]')).toBeTruthy();
});

it("shows a working seat's live counters", async () => {
  stub(busyParts());
  sheet();

  expect(await screen.findByText("M1-S1")).toBeTruthy();
  expect(screen.getByText(/run #7/)).toBeTruthy();
  // Three turns, counted twice on purpose: once in the page's own total and once on the
  // seat accruing them.
  expect(screen.getAllByText("3").length).toBe(2);
  expect(screen.getAllByText("900").length).toBe(2);
});

it("marks the slice a seat actually has open, not merely one the board calls active", async () => {
  // Those are different claims: `active` is a board status, and only one of them is
  // happening right now.
  const parts = busyParts();
  parts.board = board({
    plan: { plan: "widget", title: "Ship it", status: "active", slice: null },
    slices: [slice("M1-S1", "active", ["src/**"]), slice("M1-S2", "active", ["docs/**"])],
  });
  stub(parts);
  sheet();

  await waitFor(() => expect(document.querySelectorAll(".tape__tick").length).toBe(2));
  expect(document.querySelectorAll('.tape__tick[data-live="true"]').length).toBe(1);
});

it("falls back to a seat's territory when no slice narrows it", async () => {
  // A turn driven straight at a worktree has no slice behind it. Claiming to know the
  // file would be a guess; claiming the zone is not.
  stub({ crew: [member({ doing: "working", slice_key: null })] });
  sheet();

  await waitFor(() => expect(document.querySelector('.overview[data-busy="true"]')).toBeTruthy());
  expect(document.querySelector('[data-state="hot"]')).toBeNull();
  expect(document.querySelectorAll('[data-state="warm"]').length).toBeGreaterThan(0);
  // Territory is not a route, so nothing flows.
  expect(document.querySelector(".map__flow")).toBeNull();
});

it("reads the checkout once, not on every tick", async () => {
  // The map is a filesystem walk. Re-running it on every database tick would be hundreds
  // of `read_dir` calls a second during exactly the run it is meant to be drawing.
  stub({});
  const view = render(<Overview project={WIDGET} tick={0} onGo={() => {}} />);
  await screen.findByText("Widget");

  const mapCalls = () =>
    vi.mocked(fetch).mock.calls.filter(([url]) => String(url).startsWith("/api/map")).length;
  expect(mapCalls()).toBe(1);

  view.rerender(<Overview project={WIDGET} tick={1} onGo={() => {}} />);
  view.rerender(<Overview project={WIDGET} tick={2} onGo={() => {}} />);
  await waitFor(() =>
    expect(
      vi.mocked(fetch).mock.calls.filter(([url]) => String(url).startsWith("/api/crew")).length,
    ).toBeGreaterThan(1),
  );
  expect(mapCalls()).toBe(1);
});

it("survives a route that fails without blanking the page", async () => {
  // Every panel comes from its own request. One 500 should read as an error, not as an
  // empty repository.
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve({ ok: false, status: 500, json: async () => ({ error: "no" }) })),
  );
  sheet();
  await waitFor(() => expect(document.querySelector(".error")).toBeTruthy());
});
