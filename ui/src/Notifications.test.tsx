import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { Notifications } from "./Notifications";
import type { Notification } from "./api";

const NOTICE: Notification = {
  id: 4,
  chat_id: null,
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

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("anchors its portal beside the bell and dismisses with Escape or an outside click", async () => {
  vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: true, json: async () => ({ items: [NOTICE], unread: 1 }) })));
  render(<Notifications tick={0} onOpen={vi.fn()} />);
  const bell = await screen.findByRole("button", { name: /notifications/i });
  vi.spyOn(bell, "getBoundingClientRect").mockReturnValue({ top: 24, bottom: 56, left: 230, right: 262, width: 32, height: 32 } as DOMRect);
  await userEvent.click(bell);
  const panel = screen.getByRole("region", { name: "Notifications" });
  expect(panel.parentElement).toBe(document.body);
  expect(panel.style.top).toBe("24px");
  expect(panel.style.left).toBe("270px");
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
  expect(document.activeElement).toBe(bell);
  await userEvent.click(bell);
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
});

it("marks and clears individual alerts without navigating or forgetting their server state", async () => {
  const open = vi.fn();
  let items = [NOTICE];
  vi.stubGlobal("fetch", vi.fn((url: string) => {
    if (url.endsWith("/4/read")) items = [{ ...NOTICE, read_at: "2026-10-06" }];
    if (url.endsWith("/4/clear")) items = [];
    return Promise.resolve({ ok: true, json: async () => url.endsWith("/4/read") ? items[0] : url.endsWith("/clear") ? {} : { items, unread: items.filter(item => item.read_at === null).length } });
  }));
  render(<Notifications tick={0} onOpen={open} />);
  await userEvent.click(await screen.findByRole("button", { name: /notifications/i }));
  await userEvent.click(await screen.findByRole("button", { name: `Mark as read: ${NOTICE.title}` }));
  await screen.findByText("all caught up");
  expect(open).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: `Clear: ${NOTICE.title}` }));
  await waitFor(() => expect(screen.queryByText(NOTICE.title)).toBeNull());
  expect(fetch).toHaveBeenCalledWith("/api/notifications/4/clear", expect.objectContaining({ method: "POST" }));
  expect(open).not.toHaveBeenCalled();
});

it("does not let a late pre-clear refresh put cleared notifications back", async () => {
  let resolve!: (value: unknown) => void;
  let reads = 0;
  vi.stubGlobal("fetch", vi.fn((_url: string, init?: RequestInit) => {
    if (init?.method === "POST") return Promise.resolve({ ok: true, json: async () => ({}) });
    if (++reads === 2) return new Promise(done => { resolve = done; });
    return Promise.resolve({ ok: true, json: async () => ({ items: reads === 1 ? [NOTICE] : [], unread: reads === 1 ? 1 : 0 }) });
  }));
  const view = render(<Notifications tick={0} onOpen={vi.fn()} />);
  await screen.findByText("1");
  view.rerender(<Notifications tick={1} onOpen={vi.fn()} />);
  await userEvent.click(screen.getByRole("button", { name: /notifications/i }));
  await userEvent.click(screen.getByRole("button", { name: "Clear all" }));
  await waitFor(() => expect(screen.queryByText(NOTICE.title)).toBeNull());
  await act(async () => resolve({ ok: true, json: async () => ({ items: [NOTICE], unread: 1 }) }));
  expect(screen.queryByText(NOTICE.title)).toBeNull();
});

it("bounds bulk actions to the last observed notification and keeps failed actions visible", async () => {
  vi.stubGlobal("fetch", vi.fn((_url: string, init?: RequestInit) => Promise.resolve(init?.method === "POST"
    ? { ok: false, status: 503, json: async () => ({ error: "Inbox unavailable" }) }
    : { ok: true, json: async () => ({ items: [NOTICE], unread: 1 }) })));
  render(<Notifications tick={0} onOpen={vi.fn()} />);
  await userEvent.click(await screen.findByRole("button", { name: /notifications/i }));
  await userEvent.click(await screen.findByRole("button", { name: "Mark all as read" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Inbox unavailable");
  expect(fetch).toHaveBeenCalledWith("/api/notifications/read", expect.objectContaining({ body: JSON.stringify({ through_id: 4 }) }));
  expect(screen.getByText(NOTICE.title)).not.toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Clear all" }));
  await waitFor(() => expect(fetch).toHaveBeenCalledWith("/api/notifications/clear", expect.objectContaining({ body: JSON.stringify({ through_id: 4 }) })));
  expect(screen.getByText(NOTICE.title)).not.toBeNull();
});

it("keeps attention durable, marks it read, and opens its exact run", async () => {
  const open = vi.fn();
  vi.stubGlobal(
    "fetch",
    vi.fn((_input: string, init?: RequestInit) =>
      Promise.resolve({
        ok: true,
        json: async () =>
          init?.method === "POST" ? { ...NOTICE, read_at: "2026-09-22T14:01:00Z" } : { items: [NOTICE], unread: 1 },
      }),
    ),
  );

  render(<Notifications tick={0} onOpen={open} />);
  expect(await screen.findByText("1")).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: /notifications/i }));
  const panel = screen.getByRole("region", { name: "Notifications" });
  expect(panel.parentElement).toBe(document.body);
  await userEvent.click(screen.getByRole("button", { name: /^Widget · planner needs input/i }));

  await waitFor(() => expect(open).toHaveBeenCalledWith(expect.objectContaining({ run_id: 7 })));
  expect(fetch).toHaveBeenCalledWith(
    "/api/notifications/4/read",
    expect.objectContaining({ method: "POST" }),
  );
});
