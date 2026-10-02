import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import App from "./App";
import type { Chat } from "./chat-api";

const state = vi.hoisted(() => ({
  setup: false,
  fail: false,
  subscribe: null as (() => void) | null,
  chats: [] as Chat[],
  run: Promise.resolve({ workspace_path: "/demo-task" }),
  slice: "PR2",
  notification: { project_id: 1, workspace_path: "/demo-task", run_id: 10, chat_id: null as number | null },
}));
vi.mock("./api", async (original) => ({
  ...(await original<typeof import("./api")>()),
  projects: () =>
    state.fail
      ? Promise.reject(new Error("Server unreachable"))
      : Promise.resolve([
          {
            id: 1,
            slug: "demo",
            name: "Demo project",
            repo_path: "/demo",
            team_id: null,
          },
          {
            id: 2,
            slug: "other",
            name: "Other project",
            repo_path: "/other",
            team_id: null,
          },
        ]),
  doctor: () => Promise.resolve({ needs_setup: state.setup }),
  subscribe: (callback: () => void) => {
    state.subscribe = callback;
    return () => {
      state.subscribe = null;
    };
  },
  run: () => state.run,
  worktrees: () =>
    Promise.resolve([
      {
        name: "main",
        path: "/demo",
        main: true,
        branch: "main",
        status: "main",
      },
      {
        name: "task",
        path: "/demo-task",
        main: false,
        branch: "feature",
        status: "linked",
        kind: "pr",
        slice_key: state.slice,
        parent: "/demo",
      },
    ]),
}));
vi.mock("./chat-api", () => ({
  chats: (slug: string) => Promise.resolve(slug === "demo" ? state.chats : []),
}));
vi.mock("./Chat", () => ({
  ChatView: ({
    id,
    project,
    onCreated,
  }: {
    id: number | null;
    project: { name: string };
    onCreated: (id: number) => void;
  }) => (
    <section>
      <h1>
        {project.name} / {id ?? "New chat"}
      </h1>
      <button onClick={() => onCreated(99)}>Test creation</button>
    </section>
  ),
}));
vi.mock("./Notifications", () => ({ Notifications: ({ onOpen }: { onOpen: (notice: typeof state.notification) => void }) => <button onClick={() => onOpen(state.notification)}>Open notification</button> }));
vi.mock("./Projects", () => ({ Projects: () => <h1>Register projects</h1> }));
vi.mock("./Setup", () => ({ Setup: () => <h1>Welcome setup</h1> }));
vi.mock("./Settings", () => ({
  Settings: ({ onTheme }: { onTheme: (theme: "light") => void }) => (
    <section>
      <h1>Settings content</h1>
      <button onClick={() => onTheme("light")}>Light</button>
    </section>
  ),
}));
vi.mock("./Schedule", () => ({ Schedule: () => <h1>Schedule content</h1> }));
vi.mock("./Roster", () => ({ Roster: () => <h1>Default team</h1> }));
vi.mock("./Analytics", () => ({ Analytics: () => <h1>Analytics content</h1> }));
vi.mock("./Today", () => ({
  Today: ({
    onOpenRun,
    onOpenReview,
  }: {
    onOpenRun: (id: number, slug: string) => void;
    onOpenReview: (id: number, slug: string, run: number) => void;
  }) => (
    <section>
      <h1>Today content</h1>
      <button onClick={() => onOpenRun(10, "demo")}>Open legacy run</button>
      <button onClick={() => onOpenReview(7, "demo", 10)}>
        Open legacy review
      </button>
    </section>
  ),
}));
vi.mock("./Workspace", () => ({
  WORKSPACE_VIEWS: ["editor", "terminal", "source"],
  workspaceViewName: (value: string) => value,
  Workspace: ({
    workspace,
    view,
    openReview,
  }: {
    workspace: { path: string };
    view: string;
    openReview?: number | null;
  }) => (
    <section>
      <h1>
        {workspace.path} / {view}
      </h1>
      {openReview != null && <p>Review {openReview}</p>}
    </section>
  ),
}));

function conversation(id: number, title: string): Chat {
  return {
    id,
    mode: "single",
    title,
    project_id: 1,
    workspace_path: "/demo",
    provider: "openai",
    model: "gpt-5",
    reasoning: "high",
    active_node_id: null,
    live_text: "",
    stop_requested: false,
    archived: false,
    rev: 0,
    created_at: "2026-01-01",
    updated_at: "2026-01-01",
  };
}

beforeEach(() => {
  state.setup = false;
  state.fail = false;
  state.run = Promise.resolve({ workspace_path: "/demo-task" });
  state.slice = "PR2";
  state.notification = { project_id: 1, workspace_path: "/demo-task", run_id: 10, chat_id: null };
  state.chats = [
    conversation(1, "First conversation"),
    conversation(2, "Second conversation"),
  ];
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});

it("opens chat-first with real chats beneath their project, not a run list", async () => {
  render(<App />);
  await screen.findByRole("heading", { name: "Demo project / New chat" });
  const sidebar = screen.getByRole("navigation", {
    name: "Projects and chats",
  });
  expect(
    within(sidebar).getByRole("button", { name: "First conversation" }),
  ).not.toBeNull();
  expect(screen.queryByRole("heading", { name: "Today content" })).toBeNull();
});

it("selects a persisted chat and restores it after reload", async () => {
  const view = render(<App />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Second conversation" }),
  );
  await screen.findByRole("heading", { name: "Demo project / 2" });
  view.unmount();
  render(<App />);
  await screen.findByRole("heading", { name: "Demo project / 2" });
});

it("does not turn a just-created chat back into an empty draft before the list refreshes", async () => {
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Test creation" }));
  await screen.findByRole("heading", { name: "Demo project / 99" });
  await act(async () => state.subscribe?.());
  expect(
    screen.getByRole("heading", { name: "Demo project / 99" }),
  ).not.toBeNull();
});

it("changing project starts a scoped draft and never carries the previous chat id", async () => {
  render(<App />);
  fireEvent.click(
    await screen.findByRole("button", { name: "First conversation" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Other project" }));
  await screen.findByRole("heading", { name: "Other project / New chat" });
});

it("discards an invalid restored chat instead of displaying another project's id", async () => {
  localStorage.setItem(
    "ai-team.last-chat",
    JSON.stringify({ project: "other", chat: 1 }),
  );
  render(<App />);
  await screen.findByRole("heading", { name: "Other project / New chat" });
});

it("new chat stays in the chosen project", async () => {
  render(<App />);
  fireEvent.click(
    await screen.findByRole("button", { name: "First conversation" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "New chat" }));
  await screen.findByRole("heading", { name: "Demo project / New chat" });
});

it("preserves onboarding, project registration, and Settings entry points", async () => {
  state.setup = true;
  render(<App />);
  await screen.findByRole("heading", { name: "Welcome setup" });
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Settings content" });
  fireEvent.click(screen.getByRole("button", { name: "Add project" }));
  await screen.findByRole("heading", { name: "Register projects" });
});

it("retains project tools with explicit checkout ownership", async () => {
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Project tools" }));
  await screen.findByRole("heading", { name: "/demo / editor" });
  fireEvent.change(screen.getByRole("combobox", { name: "Project checkout" }), {
    target: { value: "/demo-task" },
  });
  fireEvent.click(screen.getByRole("button", { name: "terminal" }));
  await screen.findByRole("heading", { name: "/demo-task / terminal" });
});

it("keeps schedule and less frequent tools accessible without replacing chat navigation", async () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Schedule" }));
  await screen.findByRole("heading", { name: "Schedule content" });
  fireEvent.click(screen.getByRole("button", { name: "More tools" }));
  fireEvent.click(screen.getByRole("button", { name: "Today" }));
  await screen.findByRole("heading", { name: "Today content" });
});

it("opens legacy Today runs and reviews in their own checkout", async () => {
  render(<App />);
  await screen.findByRole("heading", { name: "Demo project / New chat" });
  fireEvent.click(screen.getByRole("button", { name: "More tools" }));
  fireEvent.click(screen.getByRole("button", { name: "Today" }));
  fireEvent.click(screen.getByRole("button", { name: "Open legacy run" }));
  await screen.findByRole("heading", { name: "/demo-task / work" });
  fireEvent.click(screen.getByRole("button", { name: "Today" }));
  fireEvent.click(screen.getByRole("button", { name: "Open legacy review" }));
  await screen.findByRole("heading", { name: "/demo-task / review" });
  expect(screen.getByText("Review 7")).not.toBeNull();
});

it("does not let a late legacy review lookup replace a newly selected chat", async () => {
  let resolve!: (value: { workspace_path: string }) => void;
  state.run = new Promise((done) => {
    resolve = done;
  });
  render(<App />);
  await screen.findByRole("heading", { name: "Demo project / New chat" });
  fireEvent.click(screen.getByRole("button", { name: "More tools" }));
  fireEvent.click(screen.getByRole("button", { name: "Today" }));
  fireEvent.click(screen.getByRole("button", { name: "Open legacy review" }));
  fireEvent.click(screen.getByRole("button", { name: "Other project" }));
  await act(async () => resolve({ workspace_path: "/demo-task" }));
  expect(
    screen.getByRole("heading", { name: "Other project / New chat" }),
  ).not.toBeNull();
});

it("opens chat notifications in their originating conversation, not legacy work", async () => {
  render(<App />);
  await screen.findByRole("button", { name: "First conversation" });
  fireEvent.click(screen.getByRole("button", { name: "Other project" }));
  state.notification.chat_id = 2;
  fireEvent.click(screen.getByRole("button", { name: "Open notification" }));
  await screen.findByRole("heading", { name: "Demo project / 2" });
  expect(screen.queryByRole("combobox", { name: "Project checkout" })).toBeNull();
});

it("still opens legacy notifications in their recorded checkout", async () => {
  render(<App />);
  await screen.findByRole("button", { name: "First conversation" });
  fireEvent.click(screen.getByRole("button", { name: "Open notification" }));
  await screen.findByRole("heading", { name: "/demo-task / work" });
});

it("keeps nested PR checkout labels current on database ticks", async () => {
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Project tools" }));
  await screen.findByRole("option", { name: "— PR2" });
  state.slice = "PR3";
  await act(async () => state.subscribe?.());
  await screen.findByRole("option", { name: "— PR3" });
});

it("refreshes persisted chat titles on a database tick", async () => {
  render(<App />);
  await screen.findByRole("button", { name: "First conversation" });
  state.chats = [conversation(1, "Renamed conversation")];
  await act(async () => state.subscribe?.());
  await screen.findByRole("button", { name: "Renamed conversation" });
  expect(
    screen.queryByRole("button", { name: "First conversation" }),
  ).toBeNull();
});

it("filters chats locally and remembers the theme through existing Settings", async () => {
  render(<App />);
  await screen.findByRole("button", { name: "First conversation" });
  fireEvent.change(screen.getByRole("textbox", { name: "Find chats" }), {
    target: { value: "Second" },
  });
  expect(
    screen.queryByRole("button", { name: "First conversation" }),
  ).toBeNull();
  expect(
    screen.getByRole("button", { name: "Second conversation" }),
  ).not.toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("button", { name: "Light" }));
  await waitFor(() =>
    expect(document.documentElement.dataset.theme).toBe("light"),
  );
});

it("reports an unreachable server instead of pretending the chat list is empty", async () => {
  state.fail = true;
  render(<App />);
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Server unreachable",
  );
});
