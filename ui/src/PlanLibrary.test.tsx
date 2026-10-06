import { render, screen, waitFor, within } from "@testing-library/react";
import type { ChatPlan } from "./plan-api";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { PlanLibrary } from "./PlanLibrary";
import type {
  PlanImportPreview,
  PlanLibrary as Board,
  PlanSourcePlan,
  PlanSourceSurvey,
} from "./plan-library-api";

afterEach(() => {
  vi.unstubAllGlobals();
});

function board(over: Partial<Board> = {}): Board {
  return {
    entries: [
      {
        plan_id: 1,
        slug: "chat-7",
        title: "Ship the widget",
        status: "active",
        summary: "Three pull requests and a migration.",
        project_id: 1,
        project_slug: "widget",
        project_name: "Widget",
        chat_id: 7,
        chat_title: "Widget planning",
        chat_archived: false,
        slices: 4,
        done: 1,
        open_questions: 1,
        updated_at: "2026-10-01T10:00:00Z",
        last_activity: "2026-10-02T09:00:00Z",
        imported: {
          source_path: "/elsewhere/planner.db",
          source_plan: "git:github.com/acme/widget/ship-the-widget",
          imported_at: "2026-10-02T08:00:00Z",
        },
      },
      {
        plan_id: 2,
        slug: "chat-9",
        title: "Retire the gadget",
        status: "draft",
        summary: null,
        project_id: 2,
        project_slug: "gadget",
        project_name: "Gadget",
        chat_id: 9,
        chat_title: "Gadget cleanup",
        chat_archived: false,
        slices: 0,
        done: 0,
        open_questions: 0,
        updated_at: "2026-09-30T10:00:00Z",
        last_activity: null,
        imported: null,
      },
    ],
    projects: [
      { id: 1, slug: "widget", name: "Widget", plans: 1 },
      { id: 2, slug: "gadget", name: "Gadget", plans: 1 },
    ],
    destinations: [
      {
        chat_id: 12,
        project_id: 1,
        project_slug: "widget",
        project_name: "Widget",
        title: "New chat",
        workspace_path: "/repo/widget",
        created_at: "2026-10-03T08:00:00Z",
      },
    ],
    detached: [],
    ...over,
  };
}

const SOURCE = {
  path: "/elsewhere/planner.db",
  bytes: 40960,
  digest: "abc123def456789",
  schema_version: 5,
  plans: 1,
};

const SOURCE_PLAN: PlanSourcePlan = {
  id: 3,
  repo_key: "git:github.com/acme/widget",
  repo_name: "widget",
  slug: "ship-the-widget",
  title: "Ship the widget",
  status: "active",
  summary: null,
  slices: 4,
  done: 1,
  open_questions: 1,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-02-01T00:00:00Z",
  already_imported: null,
};

function survey(): PlanSourceSurvey {
  return { source: SOURCE, plans: [SOURCE_PLAN] };
}

function preview(over: Partial<PlanImportPreview> = {}): PlanImportPreview {
  return {
    source: SOURCE,
    plan: SOURCE_PLAN,
    fingerprint: "f".repeat(64),
    counts: {
      sections: 8,
      slices: 4,
      slice_deps: 0,
      decisions: 1,
      questions: 2,
      gotchas: 1,
      log: 9,
      sources: 0,
      handoffs: 1,
      raw_bytes: 42,
      file_imports: 0,
      affinities: 1,
      embeddings: 0,
    },
    preserved: [
      "9 progress notes with their original dates, actors, branches and worktrees",
    ],
    warnings: [
      "2 slices arrive as history: their claim, worktree, branch, base and pull request are recorded in the plan's import record rather than re-created.",
      "1 learned branch and worktree associations are not imported.",
    ],
    evidence: [
      {
        key: "PR1",
        title: "Schema",
        status: "done",
        claimed_by: "someone-else",
        claimed_at: "2026-01-02T00:00:00Z",
        worktree_path: "/elsewhere/trees/pr1",
        branch: "widget/pr1-schema",
        base_branch: "main",
        pr_url: "https://github.com/acme/widget/pull/1",
      },
    ],
    refusal: null,
    ...over,
  };
}

function snapshot(chat: number): ChatPlan {
  return {
    chat_id: chat,
    project_id: chat === 7 ? 1 : 2,
    revision: 10,
    archived: false,
    frozen: false,
    bundle: {
      plan: {
        id: chat,
        slug: `chat-${chat}`,
        title: chat === 7 ? "Ship the widget" : "Retire the gadget",
        summary: "Owned plan document",
        status: "active",
      },
      sections: [
        {
          key: "scope",
          title: "Grounding",
          body: chat === 7 ? "Widget scope" : "Gadget scope",
          rev: 1,
        },
      ],
      slices: [
        {
          id: chat * 10,
          key: "S1",
          title: chat === 7 ? "Implement widget" : "Remove gadget",
          status: "ready",
          scope_md: "Only this plan's checkout",
          demo_md: "Run its checks",
          blocked_reason: null,
          rev: 1,
        },
      ],
      questions: [],
      decisions: [],
      gotchas: [],
      log: [],
    },
  };
}

type Call = { url: string; method: string; body: unknown };

function stub(routes: {
  board?: Board;
  survey?: PlanSourceSurvey;
  preview?: PlanImportPreview;
  sourceError?: string;
  permissions?: { archived: boolean; frozen: boolean };
}): Call[] {
  const calls: Call[] = [];
  const snapshots = new Map<number, ChatPlan>();
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      const path = String(url).replace(/^\/api/, "");
      calls.push({
        url: path,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (path.startsWith("/plan-library/source")) {
        if (routes.sourceError !== undefined) {
          return Promise.resolve({
            ok: false,
            status: 400,
            json: () => Promise.resolve({ error: routes.sourceError }),
          });
        }
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve(routes.survey),
        });
      }
      if (path.startsWith("/plan-library/preview")) {
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve(routes.preview),
        });
      }
      if (path.startsWith("/plan-library/import")) {
        return Promise.resolve({
          ok: true,
          json: () =>
            Promise.resolve({
              chat_id: 12,
              project_id: 1,
              project_slug: "widget",
              plan_id: 5,
              slug: "chat-12",
              title: "Ship the widget",
              revision: 40,
              counts: routes.preview?.counts,
              warnings: routes.preview?.warnings ?? [],
              source: SOURCE,
            }),
        });
      }
      if (path.startsWith("/plan-library")) {
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve(routes.board ?? board()),
        });
      }
      const planPath = /^\/chats\/(\d+)\/plan$/.exec(path);
      if (planPath?.[1]) {
        const id = Number(planPath[1]);
        const plan = snapshots.get(id) ?? {
          ...snapshot(id),
          ...routes.permissions,
        };
        if (init?.method === "POST") {
          const action = JSON.parse(String(init.body));
          if (action.action === "set_slice_status" && plan.bundle) {
            const slice = plan.bundle.slices.find(
              (slice) => slice.key === action.key,
            );
            if (slice) slice.status = action.status;
          }
          plan.revision += 1;
        }
        snapshots.set(id, plan);
        return Promise.resolve({
          ok: true,
          json: async () => structuredClone(plan),
        });
      }
      if (path === "/models") {
        return Promise.resolve({
          ok: true,
          json: () =>
            Promise.resolve({
              models: [
                {
                  provider: "local",
                  runtime_provider: "ailocal",
                  model: "fixture-model",
                  context: "128k",
                  max_output: "8k",
                  thinking: true,
                  images: false,
                },
              ],
              error: null,
            }),
        });
      }
      return Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ id: 13 }),
      });
    }),
  );
  return calls;
}

it("opens a project-grouped planner workspace with its board in place, not a chat redirect", async () => {
  const calls = stub({});
  const open = vi.fn();
  render(<PlanLibrary tick={0} onOpenChat={open} />);
  const navigation = await screen.findByRole("navigation", {
    name: "Plans by project",
  });
  await userEvent.click(
    within(navigation).getByRole("button", {
      name: "Open plan Ship the widget",
    }),
  );
  expect(
    await screen.findByRole("region", { name: "ready slices" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "S1 Implement widget" }),
  ).toBeTruthy();
  expect(open).not.toHaveBeenCalled();
  expect(calls.some((call) => call.url === "/chats/7/plan")).toBe(true);
});

it("lists plans from every project and opens the exact chat that owns one", async () => {
  stub({});
  const onOpenChat = vi.fn();
  render(<PlanLibrary tick={0} onOpenChat={onOpenChat} />);

  await screen.findByRole("button", { name: "Open plan Ship the widget" });
  expect(screen.getByText("Retire the gadget")).toBeDefined();
  expect(await screen.findByText("Widget · Widget planning")).toBeDefined();
  expect(
    screen
      .getByRole("progressbar", {
        name: "Ship the widget reported slice progress",
      })
      .getAttribute("value"),
  ).toBe("1");
  expect(
    screen.getByText(/not written to or kept in step with this copy/),
  ).toBeDefined();

  await userEvent.click(
    screen.getByRole("button", { name: "Open Widget planning" }),
  );
  expect(onOpenChat).toHaveBeenCalledWith("widget", 7);
});

it("asks the server for the filtered board rather than hiding rows it already has", async () => {
  const calls = stub({});
  render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Ship the widget");

  await userEvent.selectOptions(screen.getByLabelText("Project"), "widget");
  await waitFor(() =>
    expect(
      calls.some((call) => call.url === "/plan-library?project=widget"),
    ).toBe(true),
  );
  await userEvent.selectOptions(
    within(
      screen.getByRole("navigation", { name: "Plans by project" }),
    ).getByLabelText("Status"),
    "active",
  );
  await waitFor(() =>
    expect(
      calls.some(
        (call) => call.url === "/plan-library?project=widget&status=active",
      ),
    ).toBe(true),
  );
  // Filtering is a read; entering a planning workspace grants no execution authority.
  expect(calls.every((call) => call.method === "GET")).toBe(true);
});

it("keeps section drafts across Board/Plan and project switches without writing or changing chats", async () => {
  const calls = stub({});
  const open = vi.fn();
  render(<PlanLibrary tick={0} onOpenChat={open} />);
  await screen.findByRole("region", { name: "ready slices" });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Plan" }));
  await user.click(screen.getByRole("button", { name: "Edit Grounding" }));
  await user.type(
    screen.getByRole("textbox", { name: "Content" }),
    " — retained draft",
  );
  await user.click(screen.getByRole("button", { name: "Board" }));
  await user.click(
    screen.getByRole("button", { name: "Open plan Retire the gadget" }),
  );
  await screen.findByRole("button", { name: "S1 Remove gadget" });
  await user.click(
    screen.getByRole("button", { name: "Open plan Ship the widget" }),
  );
  await user.click(screen.getByRole("button", { name: "Plan" }));
  expect(
    (screen.getByRole("textbox", { name: "Content" }) as HTMLTextAreaElement)
      .value,
  ).toBe("Widget scope — retained draft");
  expect(open).not.toHaveBeenCalled();
  expect(calls.every((call) => call.method === "GET")).toBe(true);
});
it("changes a board slice through the exact owning chat and reviewed revision", async () => {
  const calls = stub({});
  render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(
    await screen.findByRole("button", { name: "S1 Implement widget" }),
  );
  const drawer = within(
    screen.getByRole("complementary", { name: "Slice S1" }),
  );
  await user.selectOptions(
    drawer.getByRole("combobox", { name: "S1 status" }),
    "active",
  );
  await user.click(drawer.getByRole("button", { name: "Update status" }));
  await waitFor(() =>
    expect(calls.filter((call) => call.method === "POST")).toEqual([
      {
        url: "/chats/7/plan",
        method: "POST",
        body: {
          action: "set_slice_status",
          expect_revision: 10,
          key: "S1",
          status: "active",
        },
      },
    ]),
  );
  await waitFor(() =>
    expect(
      within(screen.getByRole("region", { name: "active slices" })).getByRole(
        "button",
        { name: "S1 Implement widget" },
      ),
    ).toBeTruthy(),
  );
});
it.each([
  { archived: true, frozen: false },
  { archived: false, frozen: true },
])(
  "keeps the board readable while server scope prevents edits: %j",
  async (permissions) => {
    const calls = stub({ permissions });
    render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "S1 Implement widget" }),
    );
    expect(
      screen.getByRole("button", { name: "Add slice" }).matches(":disabled"),
    ).toBe(true);
    expect(
      screen.getByRole("button", { name: "Edit S1" }).matches(":disabled"),
    ).toBe(true);
    await user.click(screen.getByRole("button", { name: "Plan" }));
    expect(
      screen.getByRole("button", { name: "Edit Grounding" }).matches(":disabled"),
    ).toBe(true);
    expect(
      screen
        .getByRole("button", { name: "Ask a question" })
        .matches(":disabled"),
    ).toBe(permissions.archived);
    expect(calls.every((call) => call.method === "GET")).toBe(true);
  },
);

it("imports only what was previewed, into the one chat that was chosen", async () => {
  const calls = stub({ survey: survey(), preview: preview() });
  const onOpenChat = vi.fn();
  const onChanged = vi.fn();
  render(
    <PlanLibrary tick={0} onOpenChat={onOpenChat} onChanged={onChanged} />,
  );
  await screen.findByText("Ship the widget");

  const user = userEvent.setup();
  await user.type(
    screen.getByLabelText("Planner database"),
    "/elsewhere/planner.db",
  );
  await user.click(screen.getByRole("button", { name: "Read database" }));

  await screen.findByRole("button", { name: "Preview ship-the-widget" });
  expect(
    calls.find((call) => call.url === "/plan-library/source")?.body,
  ).toEqual({ path: "/elsewhere/planner.db" });

  await user.click(
    screen.getByRole("button", { name: "Preview ship-the-widget" }),
  );
  await screen.findByText(
    "9 progress notes with their original dates, actors, branches and worktrees",
  );
  expect(
    screen.getByText(/learned branch and worktree associations/),
  ).toBeDefined();
  expect(screen.getByText("widget/pr1-schema")).toBeDefined();
  expect(
    screen.getByText(/leases nothing, builds nothing and publishes nothing/),
  ).toBeDefined();

  // No destination chosen yet, so there is nothing to approve.
  const approve = screen.getByRole("button", { name: "Import into this chat" });
  expect(approve.hasAttribute("disabled")).toBe(true);

  await user.selectOptions(screen.getByLabelText("Import into"), "12");
  await user.click(approve);

  await waitFor(() =>
    expect(
      calls.find((call) => call.url === "/plan-library/import")?.body,
    ).toEqual({
      path: "/elsewhere/planner.db",
      plan_id: 3,
      chat_id: 12,
      fingerprint: "f".repeat(64),
    }),
  );
  expect(onChanged).toHaveBeenCalled();
  await user.click(
    await screen.findByRole("button", { name: "Open the chat" }),
  );
  expect(onOpenChat).toHaveBeenCalledWith("widget", 12);
});

it("refuses to offer an import the server has already said it will not take", async () => {
  stub({
    survey: survey(),
    preview: preview({
      refusal:
        "this exact plan was already imported on 2026-10-02 and is chat 7's plan.",
    }),
  });
  const user = userEvent.setup();
  render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Ship the widget");

  await user.type(
    screen.getByLabelText("Planner database"),
    "/elsewhere/planner.db",
  );
  await user.click(screen.getByRole("button", { name: "Read database" }));
  await user.click(
    await screen.findByRole("button", { name: "Preview ship-the-widget" }),
  );

  await screen.findByText(/already imported on 2026-10-02/);
  await user.selectOptions(screen.getByLabelText("Import into"), "12");
  expect(
    screen
      .getByRole("button", { name: "Import into this chat" })
      .hasAttribute("disabled"),
  ).toBe(true);
});

it("says what went wrong reading a database instead of silently showing nothing", async () => {
  stub({ sourceError: "cannot read /nope.db: No such file or directory" });
  const user = userEvent.setup();
  render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Ship the widget");

  await user.type(screen.getByLabelText("Planner database"), "/nope.db");
  await user.click(screen.getByRole("button", { name: "Read database" }));

  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    "cannot read /nope.db: No such file or directory",
  );
  expect(screen.queryByRole("button", { name: /^Preview/ })).toBeNull();
});

it("can create an empty destination even when another exists, without starting a turn", async () => {
  const calls = stub({
    board: board(),
    survey: survey(),
    preview: preview(),
  });
  const user = userEvent.setup();
  render(<PlanLibrary tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Ship the widget");
  await user.type(
    screen.getByLabelText("Planner database"),
    "/elsewhere/planner.db",
  );
  await user.click(screen.getByRole("button", { name: "Read database" }));
  await user.click(
    await screen.findByRole("button", { name: "Preview ship-the-widget" }),
  );

  // An existing empty chat must not force the operator to use that project/context.
  // The operator can always create the destination they actually intend.
  await screen.findByLabelText("Import into");
  expect(
    screen
      .getByRole("button", { name: "Import into this chat" })
      .hasAttribute("disabled"),
  ).toBe(true);
  await user.click(
    screen.getByRole("button", { name: "Create an empty chat" }),
  );
  await user.type(screen.getByLabelText("Project slug"), "widget");
  await waitFor(() =>
    expect(
      screen.getByRole("option", { name: "local · fixture-model" }),
    ).toBeDefined(),
  );
  await user.selectOptions(
    screen.getByLabelText("Model"),
    "local|fixture-model",
  );
  await user.selectOptions(screen.getByLabelText("Reasoning"), "high");
  await user.click(screen.getByRole("button", { name: "Create the chat" }));

  await waitFor(() =>
    expect(calls.some((call) => call.url === "/chats")).toBe(true),
  );
  const created = calls.find((call) => call.url === "/chats");
  expect(created?.method).toBe("POST");
  expect(created?.body).toEqual({
    project: "widget",
    provider: "local",
    model: "fixture-model",
    reasoning: "high",
  });
  expect(
    calls.some(
      (call) => call.url.includes("/messages") || call.url.includes("/resume"),
    ),
    "creating a chat must not start a turn",
  ).toBe(false);
});
