import { render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { ChatOverview } from "./ChatOverview";
import type { RepoMap, RunEvent } from "./api";
import type { ChatDetail } from "./chat-api";
import type { ChatOverview as Live, ChatSeat } from "./chat-overview-api";
import type { ChatPlan } from "./plan-api";

afterEach(() => {
  vi.unstubAllGlobals();
});

const NOW = Date.parse("2026-10-06T09:05:00Z");

function detail(over: Partial<ChatDetail> = {}): ChatDetail {
  return {
    mode: "single",
    id: 7,
    project_id: 1,
    title: "Add a subtract",
    workspace_path: "/tmp/widget",
    provider: "local",
    model: "fixture",
    reasoning: "high",
    active_node_id: 4,
    live_text: "",
    stop_requested: false,
    archived: false,
    rev: 3,
    created_at: "2026-10-06T09:00:00Z",
    updated_at: "2026-10-06T09:00:00Z",
    turns: [
      {
        team: null,
        members: [],
        run: {
          id: 2,
          project_id: 1,
          prompt: "Add a subtract",
          status: "running",
          trigger: "manual",
          workspace_path: "/tmp/widget",
          created_at: "2026-10-06T09:00:00Z",
          started_at: "2026-10-06T09:00:00Z",
          ended_at: null,
        },
        node: {
          id: 4,
          role: "assistant",
          provider: "local",
          model: "fixture",
          status: "running",
          attempt: 1,
          slice_key: null,
          worktree_path: "/tmp/widget",
          branch: null,
          blocked_reason: null,
          usage: { tokens_in: 100, tokens_out: 20, cache_read: 0, cache_write: 0 },
          started_at: "2026-10-06T09:00:00Z",
        },
      },
    ],
    followups: [],
    team_builds: [],
    state: "running",
    can_resume: false,
    orphan_running: false,
    ...over,
  };
}

function map(): RepoMap {
  return {
    root: "widget",
    files: 4,
    unowned: 0,
    truncated: false,
    nodes: [
      { path: "", name: "widget", dir: true, depth: 0, weight: 4, owner: "assistant" },
      { path: "src", name: "src", dir: true, depth: 1, weight: 2, owner: "assistant" },
      { path: "src/lib.rs", name: "lib.rs", dir: false, depth: 2, weight: 1, owner: "assistant" },
      { path: "README.md", name: "README.md", dir: false, depth: 1, weight: 1, owner: "assistant" },
    ],
    edges: [
      { from: 0, to: 1 },
      { from: 1, to: 2 },
      { from: 0, to: 3 },
    ],
    zones: [{ role: "assistant", name: "Assistant", zone: "**", owns: 4 }],
  };
}

function seat(over: Partial<ChatSeat> = {}): ChatSeat {
  return {
    role: "assistant",
    node_id: 4,
    run_id: 2,
    provider: "local",
    model: "fixture",
    status: "running",
    attempt: 1,
    slice_key: null,
    worktree: "/tmp/widget",
    started_at: "2026-10-06T09:00:00Z",
    ended_at: null,
    blocked_reason: null,
    live: true,
    supervised: true,
    activity: {
      kind: "tool_call",
      summary: "edit",
      detail: "src/lib.rs",
      file: "src/lib.rs",
      at: "2026-10-06T09:04:48Z",
    },
    said: "Writing the subtract and its test",
    runs: 1,
    steps: 3,
    usage: { tokens_in: 1_200, tokens_out: 300, cache_read: 0, cache_write: 0 },
    context_tokens: 12_000,
    touches: [
      { path: "src/lib.rs", tool: "edit", reads: 1, writes: 2, at: "2026-10-06T09:04:48Z" },
      { path: "README.md", tool: "read", reads: 1, writes: 0, at: "2026-10-06T09:02:00Z" },
    ],
    outside: 0,
    ...over,
  };
}

function live(over: Partial<Live> = {}): Live {
  return {
    chat_id: 7,
    project: "Widget",
    workspace: "/tmp/widget",
    mode: "single",
    seats: [seat()],
    totals: {
      turns: 1,
      steps: 3,
      usage: { tokens_in: 1_200, tokens_out: 300, cache_read: 0, cache_write: 0 },
      files_touched: 2,
      files_written: 1,
    },
    nodes_total: 1,
    nodes_read: 1,
    ...over,
  };
}

function plan(): ChatPlan {
  return {
    chat_id: 7,
    project_id: 1,
    revision: 2,
    bundle: {
      plan: { id: 1, slug: "chat-7", title: "Arithmetic", summary: null, status: "active" },
      sections: [],
      slices: [
        {
          id: 3,
          key: "S1",
          title: "Subtract",
          status: "active",
          scope_md: "",
          demo_md: null,
          blocked_reason: null,
          rev: 1,
        },
      ],
      questions: [
        { id: 9, body: "Round or truncate?", status: "open", answer: null, slice_key: null },
      ],
      decisions: [],
      gotchas: [],
      log: [],
    },
  };
}

/** Every route this panel is allowed to reach, and what it answers with. */
function stub(parts: { live?: Live; plan?: ChatPlan; mapError?: boolean } = {}) {
  const asked: string[] = [];
  const routes: Record<string, unknown> = {
    "/chats/7/overview/map": { workspace: "/tmp/widget", map: map() },
    "/chats/7/overview": parts.live ?? live(),
    "/chats/7/plan": parts.plan ?? plan(),
  };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = String(input).replace(/^\/api/, "").replace(/\?.*$/, "");
      asked.push(path);
      const body = routes[path];
      if (parts.mapError && path === "/chats/7/overview") {
        return new Promise(resolve => setTimeout(() => resolve({ ok: true, json: async () => body }), 50));
      }
      if (parts.mapError && path.endsWith("/overview/map")) {
        return Promise.resolve({ ok: false, status: 500, json: async () => ({ error: "Repository map unavailable" }) });
      }
      if (body === undefined) {
        return Promise.resolve({ ok: false, status: 404, json: async () => ({ error: path }) });
      }
      return Promise.resolve({ ok: true, json: async () => body });
    }),
  );
  return asked;
}

/** How the map is drawing one path: `hot`, `warm`, or a still picture. */
function state(container: HTMLElement, path: string): string | null {
  const node = [...container.querySelectorAll(".map__node")].find((circle) =>
    circle.querySelector("title")?.textContent?.startsWith(`${path} · `),
  );
  return node?.getAttribute("data-state") ?? null;
}

function sheet(over: Partial<ChatDetail> = {}, events: RunEvent[] = []) {
  return render(
    <ChatOverview
      detail={detail(over)}
      status="Working"
      tick={0}
      events={events}
      elapsed={288}
      now={NOW}
    />,
  );
}

it("does not hide a map failure when the live evidence succeeds", async () => {
  stub({ mapError: true });
  sheet();
  await screen.findByRole("group", { name: "Agent execution graph" });
  expect(screen.getByRole("alert").textContent).toContain("Repository map unavailable");
});

it("does not light old attempt files as current work", async () => {
  stub({ live: live({ seats: [seat({ live_touches: [], activity: null })] }) });
  const { container } = sheet();
  await screen.findByLabelText("Agents and files");
  expect(state(container, "src/lib.rs")).not.toBe("hot");
  expect(state(container, "README.md")).not.toBe("warm");
});

it("reads this chat and nothing else", async () => {
  // The regression the chat-first refactor exists to prevent: a command surface that
  // answers with the project's latest run, crew or board under another chat's heading.
  const asked = stub();
  sheet();

  await screen.findByLabelText("Agents and files");
  expect(asked.length).toBeGreaterThan(0);
  for (const path of asked) {
    expect(path.startsWith("/chats/7/"), `${path} is not scoped to this chat`).toBe(true);
  }
  expect(asked.some((path) => path.includes("project="))).toBe(false);
});

it("draws the agent on the file it is editing, and says what it is running", async () => {
  stub();
  const { container } = sheet();

  // The map's own account of itself: the node the seat is editing is lit, in that seat's
  // name, and the one it only read breathes in the quieter register beside it.
  await screen.findByLabelText("Agents and files");
  expect(screen.getByRole("img", { name: /1 working/ })).toBeTruthy();
  expect(screen.getByRole("group", { name: "Agent execution graph" })).toBeTruthy();
  expect(state(container, "src/lib.rs")).toBe("hot");
  expect(state(container, "README.md")).toBe("warm");
  // The route from the root down to the edited file carries the work.
  expect(container.querySelectorAll(".map__flow").length).toBe(2);

  const command = screen.getByLabelText("assistant current command");
  expect(command.getAttribute("aria-valuetext")).toBe("running for 12s");
  expect(screen.getByText("Current command")).toBeTruthy();
  expect(screen.getByText(/edit · src\/lib\.rs/)).toBeTruthy();
  expect(screen.getByText("Writing the subtract and its test")).toBeTruthy();
  // Operational cards carry evidence, never an invented percentage.
  expect(screen.queryByText(/%\s*complete/i)).toBeNull();
  expect(screen.getByText("2 edits")).toBeTruthy();
  expect(screen.getByText("1 read")).toBeTruthy();
});

it("leads with what the checkout is, even before anything has run in it", async () => {
  stub({
    live: live({
      seats: [],
      totals: {
        turns: 0,
        steps: 0,
        usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 },
        files_touched: 0,
        files_written: 0,
      },
      nodes_total: 0,
      nodes_read: 0,
    }),
  });
  const { container } = sheet({ active_node_id: null, turns: [], state: "empty" });

  await screen.findByLabelText("Agents and files");
  const figures = within(container.querySelector(".chat-figures") as HTMLElement);
  expect(figures.getByText("4")).toBeTruthy();
  expect(figures.getByText("100%")).toBeTruthy();
  expect(figures.getByText("0/0")).toBeTruthy();
  expect(
    screen.getByText("No agent has worked in this chat yet. Send a message and its seat appears here."),
  ).toBeTruthy();
  // Nothing is working, so the picture is still rather than ambiently animated.
  expect(screen.getByRole("img", { name: /nothing running/ }).getAttribute("data-busy")).toBe(
    "false",
  );
});

it("an interrupted seat keeps its evidence and stops pretending to work", async () => {
  stub({ live: live({ seats: [seat({ supervised: false })] }) });
  const { container } = sheet();

  expect(await screen.findByText("no live process")).toBeTruthy();
  expect(screen.queryByLabelText("assistant current command")).toBeNull();
  // Its files are still what it touched; only the claim that it is working is withdrawn.
  expect(screen.getByText("src/lib.rs")).toBeTruthy();
  expect(state(container, "src/lib.rs")).toBe("still");
  expect(screen.getByRole("img", { name: /nothing running/ })).toBeTruthy();
});

it("reports the plan this chat owns, and says when there is none", async () => {
  stub();
  const { unmount } = sheet();
  expect(await screen.findByText("Arithmetic")).toBeTruthy();
  expect(screen.getByText("1 open question · 1 slice")).toBeTruthy();
  unmount();

  stub({ plan: { chat_id: 7, project_id: 1, revision: 0, bundle: null } });
  sheet();
  expect(
    await screen.findByText(
      "No plan in this chat yet. Ask for one in the conversation, or open Board to write it.",
    ),
  ).toBeTruthy();
});

it("says how much of the conversation the file evidence was read from", async () => {
  stub({ live: live({ nodes_total: 40, nodes_read: 24 }) });
  sheet();

  await waitFor(() =>
    expect(
      screen.getByText("Files and totals are read from the newest 24 of 40 agent turns in this chat."),
    ).toBeTruthy(),
  );
});
