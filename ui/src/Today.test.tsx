import { render, screen, within, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { Today } from "./Today";
import type { ChatToday, ChatTodayEntry } from "./chat-today-api";

afterEach(() => vi.unstubAllGlobals());
function entry(over: Partial<ChatTodayEntry> = {}): ChatTodayEntry {
  return {
    chat_id: 7,
    project_slug: "widget",
    project_name: "Widget",
    title: "My chat",
    workspace_path: "/repo/linked",
    updated_at: "2026-10-06T10:00:00Z",
    state: "failed",
    detail: "Check failed",
    panel: "work",
    needs_attention: true,
    working: false,
    drafts: 0,
    questions: 0,
    ...over,
  };
}
function data(entries: ChatTodayEntry[]): ChatToday {
  return {
    entries,
    chats: entries.length,
    needs_attention: entries.filter((item) => item.needs_attention).length,
    working: entries.filter((item) => item.working).length,
    drafts: entries.reduce((sum, item) => sum + item.drafts, 0),
  };
}
function stub(body: ChatToday) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = String(input).replace(/^\/api/, "");
      calls.push(path);
      // A legacy leak must be visible, not masked by a permissive empty-array mock.
      const response =
        path === "/chat-today"
          ? body
          : [
              {
                id: 99,
                project_id: 1,
                prompt: "legacy work unrelated to chats",
                status: "blocked",
              },
            ];
      return Promise.resolve({ ok: true, json: async () => response });
    }),
  );
  return calls;
}
it("does not query legacy feeds or count old runs when there are no active chats", async () => {
  const calls = stub(data([]));
  render(<Today tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Nothing is waiting on you");
  expect(calls).toEqual(["/chat-today"]);
  expect(screen.queryByText("legacy work unrelated to chats")).toBeNull();
  for (const label of [
    "Needs you",
    "Chats working",
    "Drafts to review",
    "Open chats",
  ])
    expect(screen.getByText(label).previousElementSibling?.textContent).toBe(
      "0",
    );
  expect(screen.queryByText("Throughput")).toBeNull();
  expect(screen.queryByText("Active runs")).toBeNull();
});
it("keeps the chat-owned server ranking rather than resurfacing older attempts", async () => {
  stub(
    data([
      entry({ title: "first" }),
      entry({ chat_id: 8, title: "second", state: "inspection" }),
      entry({
        chat_id: 9,
        title: "third",
        state: "running",
        needs_attention: false,
        working: true,
      }),
    ]),
  );
  const { container } = render(<Today tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Do this first");
  expect(
    [...container.querySelectorAll(".today-row strong")].map(
      (node) => node.textContent,
    ),
  ).toEqual(["first", "second", "third"]);
});
it("opens the exact owning chat and contextual panel, never a project latest run", async () => {
  stub(
    data([
      entry({
        chat_id: 17,
        title: "Inspect my draft",
        panel: "review",
        state: "review",
        drafts: 2,
      }),
    ]),
  );
  const open = vi.fn();
  render(<Today tick={0} onOpenChat={open} />);
  await userEvent.click(await screen.findByRole("button", { name: /chat 17/ }));
  expect(open).toHaveBeenCalledWith("widget", 17, "review");
  expect(screen.getByText(/review is not publication approval/)).toBeTruthy();
});
it("uses the same response for counters and clearly bounds the displayed detail", async () => {
  const response = {
    ...data([
      entry({ working: true }),
      entry({ chat_id: 8, state: "review", drafts: 2, questions: 1 }),
    ]),
    chats: 210,
    needs_attention: 20,
    working: 3,
    drafts: 12,
  };
  stub(response);
  render(<Today tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Do this first");
  const counts = within(
    screen.getByRole("region", { name: "Chat activity counts" }),
  );
  expect(
    counts.getByText("Needs you").previousElementSibling?.textContent,
  ).toBe("20");
  expect(
    counts.getByText("Chats working").previousElementSibling?.textContent,
  ).toBe("3");
  expect(
    counts.getByText("Drafts to review").previousElementSibling?.textContent,
  ).toBe("12");
  expect(screen.getByText(/Showing 2 of 210 open chats/)).toBeTruthy();
});
it("labels idle, queued and interrupted activity without treating it all as working", async () => {
  stub(
    data([
      entry({ state: "interrupted" }),
      entry({ chat_id: 8, state: "starting", needs_attention: false }),
      entry({ chat_id: 9, state: "idle", needs_attention: false }),
    ]),
  );
  render(<Today tick={0} onOpenChat={vi.fn()} />);
  await screen.findByText("Do this first");
  for (const label of ["Interrupted", "Starting / waiting", "Idle"])
    expect(screen.getAllByText(label).length).toBeGreaterThan(0);
  expect(
    screen.getByText("Chats working").previousElementSibling?.textContent,
  ).toBe("0");
});
it("shows read failures instead of a misleading clear day", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValue({
        ok: false,
        status: 503,
        json: async () => ({ error: "could not read chat activity" }),
      }),
  );
  render(<Today tick={0} onOpenChat={vi.fn()} />);
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    "could not read chat activity",
  );
  expect(screen.queryByText("Nothing is waiting on you")).toBeNull();
});
it("does not replace a newer tick with a late response", async () => {
  let old!: (value: unknown) => void;
  const first = new Promise((resolve) => {
    old = resolve;
  });
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockReturnValueOnce(first)
      .mockResolvedValue({ ok: true, json: async () => data([]) }),
  );
  const view = render(<Today tick={0} onOpenChat={vi.fn()} />);
  view.rerender(<Today tick={1} onOpenChat={vi.fn()} />);
  await screen.findByText("Nothing is waiting on you");
  await act(async () =>
    old({
      ok: true,
      json: async () => data([entry({ title: "stale response" })]),
    }),
  );
  expect(screen.queryByText("stale response")).toBeNull();
});
