import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { TerminalPane } from "./Terminal";

afterEach(() => {
  vi.unstubAllGlobals();
});

type Call = { url: string; method: string; body: unknown };

function stub(sessions: unknown[], chunks: unknown[]) {
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
        return Promise.resolve({ ok: true, json: async () => ({ id: 42 }) });
      }
      if (/^\/terminals\/\d+\?/.test(url)) {
        const chunk = chunks[Math.min(next++, chunks.length - 1)];
        return Promise.resolve({ ok: true, json: async () => chunk });
      }
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
  const calls = stub([], [
    { text: "hello ", cursor: 6, done: false, status: null },
    { text: "world", cursor: 11, done: true, status: 0 },
  ]);
  render(<TerminalPane project="widget" node={null} />);

  await waitFor(() => {
    const reads = calls.filter((call) => /^\/terminals\/\d+\?/.test(call.url));
    expect(reads.some((call) => call.url.includes("cursor=6"))).toBe(true);
  });
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
