import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { TerminalPane } from "./Terminal";
import { setSynth } from "./ui-preferences";

// Exercise the real shared controller; canvas painting alone is stubbed.
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  rows = 24; cols = 80; options = {}; buffer = { active: { viewportY: 0 } };
  open() {} loadAddon() {} write() {} dispose() {} onData() {}
  onSelectionChange() { return { dispose() {} }; }
  getSelection() { return ""; } getSelectionPosition() { return undefined; }
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: class { onContextLoss() {} dispose() {} } }));

afterEach(() => {
  setSynth(false);
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

type Call = { url: string; method: string; body: unknown };

function stub(sessions: unknown[], chunks: unknown[], closeError?: string) {
  const calls: Call[] = [];
  let next = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      const url = String(input).replace(/^\/api/, "");
      calls.push({
        url,
        method: init?.method ?? "GET",
        body: init?.body === undefined ? null : JSON.parse(String(init.body)),
      });
      if (url.startsWith("/terminals?")) {
        return Promise.resolve({ ok: true, json: async () => sessions });
      }
      if (url === "/terminals") {
        sessions = [...sessions, { id: 42, worktree: "/w", done: false, status: null }];
        return Promise.resolve({ ok: true, json: async () => ({ id: 42 }) });
      }
      if (url.startsWith("/terminals/pads")) return Promise.resolve({ ok: true, json: async () => [] });
      if (url.startsWith("/terminals/commands")) return Promise.resolve({ ok: true, json: async () => ({ worktree: "/w", running: [], history: [] }) });
      if (/^\/terminals\/\d+\?/.test(url)) {
        const chunk = chunks[Math.min(next++, chunks.length - 1)];
        return Promise.resolve({ ok: true, json: async () => chunk });
      }
      if (url.endsWith("/close") && closeError) return Promise.resolve({ ok: false, status: 400, json: async () => ({ error: closeError }) });
      return Promise.resolve({ ok: true, json: async () => ({}) });
    }),
  );
  return calls;
}

it("rejoins a session this worktree already has", async () => {
  // Closing the window must not orphan a `cargo test` four minutes in, and opening a
  // second terminal beside the live one is the same mistake with extra steps.
  const calls = stub([{ id: 7, worktree: "/w", done: false, status: null }], [
    { text: "", cursor: 0, done: false, status: null },
  ]);
  render(<TerminalPane project="widget" node={null} />);

  await waitFor(() => expect(screen.getByText("session 7")).toBeDefined());
  expect(calls.some((call) => call.url === "/terminals" && call.method === "POST")).toBe(false);
});

it("names its session under the heading, as other pages name their checkout", async () => {
  // Heading, session and Restart were three children spaced apart, so the session label
  // floated alone in the middle of the header.
  stub([{ id: 7, worktree: "/w", done: false, status: null }], [
    { text: "", cursor: 0, done: false, status: null },
  ]);
  render(<TerminalPane project="widget" node={null} />);

  const session = await screen.findByText("session 7");
  expect(session.parentElement).toBe(screen.getByRole("heading", { name: "Terminal" }).parentElement);
  expect(session.parentElement?.classList.contains("main__header")).toBe(false);
});

it("keeps the terminal mounted and explains a close that has not drained", async () => {
  const calls = stub([{ id: 7, worktree: "/w", done: false, status: null }], [
    { text: "", cursor: 0, done: false, status: null },
  ], "terminal has not drained; protection retained");
  render(<TerminalPane project="widget" node={null} />);
  await screen.findByText("session 7");
  fireEvent.click(screen.getByRole("button", { name: "Restart" }));
  await screen.findByText(/protection retained/);
  expect(screen.getByText("session 7")).toBeTruthy();
  expect(calls.some(call => call.url === "/terminals" && call.method === "POST")).toBe(false);
});

it("opens one when there is none to rejoin", async () => {
  const calls = stub([], [{ text: "", cursor: 0, done: false, status: null }]);
  render(<TerminalPane project="widget" node={null} />);

  await waitFor(() => expect(screen.getByText("session 42")).toBeDefined());
  expect(calls.some((call) => call.url === "/terminals" && call.method === "POST")).toBe(true);
});

it("does not rejoin a session that has already exited", async () => {
  // A dead terminal looks like a live one in the list; attaching to it gives a pane that
  // never responds.
  const calls = stub([{ id: 7, worktree: "/w", done: true, status: 0 }], [
    { text: "", cursor: 0, done: false, status: null },
  ]);
  render(<TerminalPane project="widget" node={null} />);

  await waitFor(() => expect(screen.getByText("session 42")).toBeDefined());
  expect(calls.some((call) => call.url === "/terminals" && call.method === "POST")).toBe(true);
});

it("reads from an absolute cursor rather than replaying everything", async () => {
  vi.useFakeTimers();
  const calls = stub([], [
    { text: "hello ", cursor: 6, done: false, status: null },
    { text: "world", cursor: 11, done: true, status: 0 },
  ]);
  await act(async () => { render(<TerminalPane project="widget" node={null} />); });
  await act(async () => { await vi.advanceTimersByTimeAsync(120); });
  const reads = () => calls.filter((call) => /^\/terminals\/\d+\?/.test(call.url)).map(call => call.url);
  expect(reads()).toEqual(["/terminals/42?cursor=0", "/terminals/42?cursor=6"]);
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(reads()).toHaveLength(2);
});

it("asks for a project before opening anything", async () => {
  const calls = stub([], []);
  render(<TerminalPane project={null} node={null} />);
  expect(screen.getByText(/Pick a project/)).toBeDefined();
  expect(calls).toHaveLength(0);
});

it("says what is wrong rather than showing a dead pane", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: false,
      status: 400,
      json: async () => ({ error: "that project has no checkout to open" }),
    }),
  );
  render(<TerminalPane project="widget" node={null} />);
  expect(await screen.findByText(/no checkout/)).toBeDefined();
});

it("hands the checkout to whichever skin is on, and neither starts a second session", async () => {
  // The sessions live in the server, so switching skins changes what is painting and
  // nothing about what is running: a four-minute build keeps going, and the pane that
  // takes over rejoins it rather than opening another shell beside it.
  const calls = stub([{ id: 7, worktree: "/w", done: false, status: null, kind: "shell", label: "Shell", duration_ms: 1 }], [
    { text: "", cursor: 0, done: false, status: null, dropped: 0 },
  ]);
  const view = render(<TerminalPane project="widget" node={null} />);
  await screen.findByText("session 7");

  act(() => { setSynth(true); });
  view.rerender(<TerminalPane project="widget" node={null} />);
  await screen.findByRole("tab", { name: /agent/i });
  expect(screen.queryByText("session 7")).toBeNull();

  act(() => { setSynth(false); });
  view.rerender(<TerminalPane project="widget" node={null} />);
  await screen.findByText("session 7");
  expect(calls.some((call) => call.url === "/terminals" && call.method === "POST")).toBe(false);
});
