import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Notifications } from "./Notifications";

const NOTICE = {
  id: 4,
  project_id: 2,
  workspace_path: "/repo/task",
  run_id: 7,
  node_run_id: 11,
  kind: "input_required",
  title: "Widget · planner needs input",
  body: "Choose an acceptance criterion.",
  action_path: null,
  read_at: null,
  delivered_at: null,
  created_at: "2026-09-22T14:00:00Z",
};

afterEach(() => vi.unstubAllGlobals());

it("keeps attention durable, marks it read, and opens its exact run", async () => {
  const open = vi.fn();
  vi.stubGlobal(
    "fetch",
    vi.fn((_input: string, init?: RequestInit) =>
      Promise.resolve({
        ok: true,
        json: async () =>
          init?.method === "POST" ? { ...NOTICE, read_at: "2026-09-22T14:01:00Z" } : [NOTICE],
      }),
    ),
  );

  render(<Notifications tick={0} onOpen={open} />);
  expect(await screen.findByText("1")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: /notifications/i }));
  const panel = screen.getByRole("region", { name: "Notifications" });
  expect(panel.parentElement).toBe(document.body);
  await userEvent.click(screen.getByRole("button", { name: /planner needs input/i }));

  await waitFor(() => expect(open).toHaveBeenCalledWith(expect.objectContaining({ run_id: 7 })));
  expect(fetch).toHaveBeenCalledWith(
    "/api/notifications/4/read",
    expect.objectContaining({ method: "POST" }),
  );
});
