import { expect, it } from "vitest";

import type { RepoMap } from "./api";
import type { ChatSeat } from "./chat-overview-api";
import { chatActivity, elapsedSince, isWorking, operational } from "./chat-overview";

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
    worktree: null,
    started_at: "2026-10-06T09:00:00Z",
    ended_at: null,
    blocked_reason: null,
    live: true,
    supervised: true,
    activity: null,
    said: null,
    runs: 1,
    steps: 3,
    usage: { tokens_in: 100, tokens_out: 20, cache_read: 0, cache_write: 0 },
    context_tokens: 4_000,
    touches: [],
    outside: 0,
    ...over,
  };
}

it("lights only the files a working seat actually touched", () => {
  const activity = chatActivity(map(), [
    seat({
      touches: [
        { path: "src/lib.rs", tool: "edit", reads: 1, writes: 2, at: "2026-10-06T09:01:00Z" },
        { path: "README.md", tool: "read", reads: 1, writes: 0, at: "2026-10-06T09:02:00Z" },
      ],
    }),
  ]);

  // A file it changed is work; one it only read is context, and reads as the quieter of
  // the two registers rather than as the same claim.
  expect(activity.hot.get(2)).toBe("assistant");
  expect(activity.warm.get(3)).toBe("assistant");
  expect(activity.hot.has(3)).toBe(false);
  expect([...activity.working]).toEqual(["assistant"]);

  // The route from the root down to the edited file carries the work, so the picture
  // reads as something travelling rather than as two unrelated dots.
  expect(activity.live.get(1)).toBe("assistant");
  expect(activity.live.get(0)).toBe("assistant");
  expect(activity.live.has(2)).toBe(false);
});

it("the file a seat is on right now outranks what it merely read", () => {
  const activity = chatActivity(map(), [
    seat({
      activity: {
        kind: "tool_call",
        summary: "read",
        detail: "README.md",
        file: "README.md",
        at: "2026-10-06T09:03:00Z",
      },
      touches: [{ path: "README.md", tool: "read", reads: 3, writes: 0, at: "2026-10-06T09:03:00Z" }],
    }),
  ]);

  expect(activity.hot.get(3)).toBe("assistant");
  expect(activity.warm.has(3)).toBe(false);
});

it("a settled or interrupted chat is a still picture", () => {
  const settled = chatActivity(map(), [
    seat({
      status: "done",
      live: false,
      supervised: null,
      touches: [{ path: "src/lib.rs", tool: "edit", reads: 0, writes: 1, at: "2026-10-06T09:01:00Z" }],
    }),
  ]);
  expect(settled.hot.size).toBe(0);
  expect(settled.working.size).toBe(0);

  // A row nothing is behind is interrupted, not working. Animating it would say the
  // opposite of what the evidence does.
  const orphaned = chatActivity(map(), [
    seat({
      supervised: false,
      touches: [{ path: "src/lib.rs", tool: "edit", reads: 0, writes: 1, at: "2026-10-06T09:01:00Z" }],
    }),
  ]);
  expect(orphaned.hot.size).toBe(0);
  expect(isWorking(seat({ supervised: false }))).toBe(false);
});

it("a path the map does not have is never drawn onto one that it does", () => {
  const activity = chatActivity(map(), [
    seat({
      touches: [{ path: "crates/other.rs", tool: "write", reads: 0, writes: 1, at: "2026-10-06T09:01:00Z" }],
    }),
  ]);
  expect(activity.hot.size).toBe(0);
  expect(activity.live.size).toBe(0);
  // It is still a working seat; the picture simply has nothing true to say about where.
  expect([...activity.working]).toEqual(["assistant"]);
});

it("an elapsed figure reads the server's UTC timestamps as UTC", () => {
  const at = "2026-10-06T09:00:00Z";
  const now = Date.parse(at) + 75_000;
  expect(elapsedSince(at, now)).toBe("1m 15s");
  // SQLite's own spelling has no zone. Read as local time it would be hours out, and west
  // of UTC it would be negative.
  expect(elapsedSince("2026-10-06 09:00:00", now)).toBe("1m 15s");
  expect(elapsedSince("not a time", now)).toBe("unknown");
});

it("a command says how long it has been running, never how far through it is", () => {
  const at = "2026-10-06T09:00:00Z";
  const now = Date.parse(at) + 12_000;
  const live = operational(
    seat({ activity: { kind: "tool_call", summary: "bash", detail: "cargo test", file: null, at } }),
    now,
  );
  expect(live).toEqual({
    label: "Current command",
    summary: "bash",
    detail: "cargo test",
    timing: "running for 12s",
    live: true,
  });

  const finished = operational(
    seat({
      status: "done",
      live: false,
      supervised: null,
      activity: { kind: "note", summary: "Added the subtract", detail: null, file: null, at },
    }),
    now,
  );
  expect(finished?.label).toBe("Latest update");
  expect(finished?.timing).toBe("12s ago");
  expect(finished?.live).toBe(false);
});
