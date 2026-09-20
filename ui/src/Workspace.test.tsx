import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Workspace } from "./Workspace";
import type { Project } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

const WIDGET: Project = {
  id: 1,
  slug: "widget",
  name: "Widget",
  kind: "repo",
  status: "active",
  open_runs: 1,
} as Project;

const RUN = {
  id: 7,
  project_id: 1,
  prompt: "add subtract",
  status: "running",
  trigger: "manual",
  created_at: "",
  started_at: null,
  ended_at: null,
};

function stub(over: Record<string, unknown> = {}) {
  const calls: string[] = [];
  const routes: Record<string, unknown> = {
    "/runs": [RUN],
    "/runs/7": {
      ...RUN,
      nodes: [
        {
          id: 1,
          role: "backend",
          provider: "claude",
          model: "sonnet",
          status: "running",
          attempt: 1,
          slice_key: "S1",
          branch: "ai-team/s1",
          blocked_reason: null,
        },
      ],
      usage: { tokens_in: 1, tokens_out: 2, cache_read: 0, cache_write: 0 },
    },
    "/runs/7/events": [{ id: 1, kind: "note", actor: null, summary: "did the thing", at: "" }],
    "/runs/7/approvals": [],
    ...over,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = String(input).replace(/^\/api/, "");
      calls.push(path);
      const body = routes[path.replace(/\?.*$/, "")];
      if (body === undefined) {
        return Promise.resolve({ ok: false, status: 404, json: async () => ({ error: path }) });
      }
      return Promise.resolve({ ok: true, json: async () => body });
    }),
  );
  return calls;
}

function area(view: Parameters<typeof Workspace>[0]["view"] = "work", openRun: number | null = null) {
  return render(
    <Workspace
      project={WIDGET}
      view={view}
      tick={0}
      openRun={openRun}
      onOpenedRun={() => {}}
      onChanged={() => {}}
    />,
  );
}

it("asks only for this project's runs", async () => {
  // A run belongs to a project. A list that spans them cannot say what pressing anything
  // would do.
  const calls = stub();
  area();
  await waitFor(() => expect(calls.some((path) => path.startsWith("/runs?"))).toBe(true));
  expect(calls.find((path) => path.startsWith("/runs?"))).toContain("project=1");
});

it("opens a run in the dock and closes it with Escape", async () => {
  const user = userEvent.setup();
  stub();
  area();

  await user.click(await screen.findByText("add subtract"));
  expect(await screen.findByText("Run #7")).toBeDefined();
  expect(await screen.findByText("ai-team/s1")).toBeDefined();
  expect(await screen.findByText("did the thing")).toBeDefined();

  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByText("Run #7")).toBeNull());
});

it("Escape closes the expanded log before the dock", async () => {
  // Innermost first, which is the rule the whole shell follows (M3-S11).
  const user = userEvent.setup();
  stub();
  area();

  await user.click(await screen.findByText("add subtract"));
  await user.click(await screen.findByText("Expand"));
  await screen.findByRole("heading", { name: "Run #7" });

  await user.keyboard("{Escape}");
  // The dock survives the first Escape.
  await waitFor(() => expect(screen.queryByText("Expand")).not.toBeNull());
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByText("Run #7")).toBeNull());
});

it("opens the run Today handed over", async () => {
  stub();
  area("work", 7);
  expect(await screen.findByText("Run #7")).toBeDefined();
});

it("says what to do when nothing has run here", async () => {
  stub({ "/runs": [] });
  area();
  expect(await screen.findByText(/Nothing has run here yet/)).toBeDefined();
});

it("each view is about this project and takes it as a fact", async () => {
  // Not a filter set somewhere else: "the board" across four projects is not a question
  // with an answer (D18).
  const calls = stub({
    "/board": { plan: null, slices: [], next_step: "Run `aip new`." },
  });
  area("board");
  await waitFor(() => expect(calls.some((path) => path.startsWith("/board?"))).toBe(true));
  expect(calls.find((path) => path.startsWith("/board?"))).toContain("project=widget");
});

it("a bad approvals response does not blank a dock that could render everything else", async () => {
  // M3-S12: they are supplementary, and one failing response must not take the panel with
  // it.
  const user = userEvent.setup();
  stub({ "/runs/7/approvals": undefined });
  area();

  await user.click(await screen.findByText("add subtract"));
  expect(await screen.findByText("Run #7")).toBeDefined();
  expect(await screen.findByText("did the thing")).toBeDefined();
});
