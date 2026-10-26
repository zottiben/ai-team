import { expect, it } from "vitest";

import type { ChatOverview, ChatSeat } from "../../../chat-overview-api";
import { laneOf, pattern } from "./AgentMonitor";
import { bars } from "./WaveMinimap";
import { letters } from "./FileBrowser";
import { indentOf } from "./StatusBar";
import { elapsedSeconds, overlapping, presenceOf, presentAt, RECENT_MS } from "./presence";

const NOW = Date.parse("2026-10-15T12:00:00Z");
const ago = (ms: number) => new Date(NOW - ms).toISOString();

function seat(over: Partial<ChatSeat> = {}): ChatSeat {
  return {
    role: "assistant",
    node_id: 1,
    run_id: 1,
    provider: "local",
    model: "fixture",
    status: "running",
    attempt: 1,
    slice_key: null,
    worktree: "/repo",
    started_at: ago(30_000),
    ended_at: null,
    blocked_reason: null,
    live: true,
    supervised: true,
    activity: { kind: "tool_call", summary: "read", detail: "src/lib.rs", file: "src/lib.rs", at: ago(1000) },
    said: null,
    runs: 1,
    steps: 3,
    usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 },
    context_tokens: null,
    touches: [],
    outside: 0,
    ...over,
  };
}

function overview(seats: ChatSeat[]): ChatOverview {
  return {
    chat_id: 1,
    project: "widget",
    workspace: "/repo",
    mode: "single",
    seats,
    totals: {
      turns: 1,
      steps: 1,
      usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 },
      files_touched: 1,
      files_written: 0,
    },
    nodes_total: 1,
    nodes_read: 1,
  };
}

it("nobody is present without an overview", () => {
  const presence = presenceOf(null, NOW);
  expect(presence.working).toBe(false);
  expect(presence.recent).toEqual([]);
  expect(elapsedSeconds(presence, NOW)).toBeNull();
});

it("a seat with no process behind it is not working", () => {
  // `live` only says the turn row is open. Something has to be running.
  const presence = presenceOf(overview([seat({ supervised: false })]), NOW);
  expect(presence.working).toBe(false);
});

it("only the last five minutes count as presence", () => {
  const presence = presenceOf(
    overview([
      seat({
        touches: [
          { path: "src/fresh.rs", tool: "read", reads: 1, writes: 0, at: ago(60_000) },
          { path: "src/stale.rs", tool: "read", reads: 1, writes: 0, at: ago(RECENT_MS + 1000) },
        ],
      }),
    ]),
    NOW,
  );
  expect(presence.recent.map(touch => touch.path)).toEqual(["src/fresh.rs"]);
  expect(presence.touched.map(touch => touch.path)).toContain("src/stale.rs");
  expect(presentAt(presence, "src/fresh.rs", false)).toBe(true);
  expect(presentAt(presence, "src/stale.rs", false)).toBe(false);
  // A folder lights when something under it does.
  expect(presentAt(presence, "src", true)).toBe(true);
  expect(presentAt(presence, "ui", true)).toBe(false);
});

it("one row per path, keeping the newest touch", () => {
  const presence = presenceOf(
    overview([
      seat({
        touches: [
          { path: "src/lib.rs", tool: "read", reads: 1, writes: 0, at: ago(200_000) },
          { path: "src/lib.rs", tool: "edit", reads: 0, writes: 1, at: ago(2000) },
        ],
      }),
    ]),
    NOW,
  );
  expect(presence.recent).toHaveLength(1);
  expect(presence.recent[0]?.tool).toBe("edit");
});

it("overlap is the open files the agent also touched", () => {
  const presence = presenceOf(
    overview([
      seat({
        touches: [
          { path: "src/lib.rs", tool: "edit", reads: 0, writes: 1, at: ago(1000) },
          { path: "src/other.rs", tool: "read", reads: 1, writes: 0, at: ago(1000) },
        ],
      }),
    ]),
    NOW,
  );
  expect(overlapping(presence, ["src/lib.rs"]).map(touch => touch.path)).toEqual(["src/lib.rs"]);
});

it("elapsed comes from the turn's own start, and is unknown when nothing runs", () => {
  expect(elapsedSeconds(presenceOf(overview([seat()]), NOW), NOW)).toBe(30);
  expect(elapsedSeconds(presenceOf(overview([seat({ live: false })]), NOW), NOW)).toBeNull();
  expect(elapsedSeconds(presenceOf(overview([seat({ started_at: null })]), NOW), NOW)).toBeNull();
});

it("no tool reports a line, so the editor is never told where to draw a caret", () => {
  // The honest half of §11.2: the tag falls back to the top of the viewport because
  // there is no position to put it at, not because the lookup failed.
  expect(presenceOf(overview([seat()]), NOW).line).toBeNull();
});

it("tools land in their own lane, and an unknown one is not guessed into one", () => {
  expect(laneOf("read")).toBe("read");
  expect(laneOf("edit")).toBe("edit");
  expect(laneOf("bash")).toBe("bash");
  expect(laneOf("clickup__get_task")).toBe("mcp");
  expect(laneOf("something_new")).toBe("other");
});

it("the mini pattern is the last steps in order, right-aligned", () => {
  const touches = [
    { path: "a", tool: "read", reads: 1, writes: 0, at: "2026-10-15T11:59:00Z" },
    { path: "b", tool: "edit", reads: 0, writes: 1, at: "2026-10-15T11:59:30Z" },
  ];
  const lanes = pattern(touches);
  expect(lanes.read[22]).toBe(true);
  expect(lanes.edit[23]).toBe(true);
  expect(lanes.bash.some(Boolean)).toBe(false);
});

it("the minimap buckets a long file and keeps every marker in the bucket", () => {
  const lengths = Array.from({ length: 1800 }, () => 10);
  const rows = bars(lengths, new Set([1001]), new Set());
  expect(rows.length).toBeLessThanOrEqual(600);
  expect(rows.filter(bar => bar.added)).toHaveLength(1);
  // Width is capped at 27 characters, so one long line does not scale the rest away.
  expect(bars([100], new Set(), new Set())[0]?.length).toBe(27);
});

it("git letters prefer a delete over a modification, and name untracked files", () => {
  const diff = (path: string, status: "added" | "modified" | "removed") => ({
    path,
    old_path: null,
    status,
    binary: false,
    hunks: [],
    additions: 0,
    deletions: 0,
  });
  const map = letters({
    branch: "main",
    branches: ["main"],
    staged: [diff("a.rs", "added"), diff("c.rs", "removed")],
    unstaged: [diff("b.rs", "modified"), diff("c.rs", "modified")],
    untracked: ["new.rs"],
  });
  expect(map.get("a.rs")).toBe("A");
  expect(map.get("b.rs")).toBe("M");
  expect(map.get("c.rs")).toBe("D");
  expect(map.get("new.rs")).toBe("U");
  expect(letters(null).size).toBe(0);
});

it("indentation is measured, and unknown when the file has none", () => {
  expect(indentOf("fn a() {\n    let x = 1;\n}\n")).toBe("4 spaces");
  expect(indentOf("fn a() {\n  let x = 1;\n}\n")).toBe("2 spaces");
  expect(indentOf("fn a() {\n\tlet x = 1;\n}\n")).toBe("Tabs");
  expect(indentOf("one\ntwo\n")).toBe("Unknown");
});
