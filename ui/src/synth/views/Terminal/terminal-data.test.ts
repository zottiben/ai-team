import { expect, it } from "vitest";

import type { RunEvent } from "../../../api";
import { agentShell } from "./agent-shell";
import { createRateSampler } from "./rate";

function event(id: number, kind: string, tool: Partial<NonNullable<RunEvent["tool"]>>): RunEvent {
  return {
    id,
    node_run_id: 1,
    session_id: "session-1",
    kind,
    actor: "agent",
    summary: "tool",
    message: null,
    thinking: [],
    at: "2026-10-15T09:00:00Z",
    tool: {
      call_id: "c1",
      parent_call_id: null,
      name: "bash",
      target: null,
      outcome: null,
      duration_ms: null,
      output: null,
      ...tool,
    },
  };
}

it("builds the mirror from the call and its return, not from whichever row is last", () => {
  const commands = agentShell([
    event(1, "tool_call", { target: { kind: "command", text: "cargo test", truncated: false } }),
    event(2, "note", { partial: true, output: { text: "running 4 tests", truncated: false } }),
    event(3, "tool_result", { outcome: "failed", duration_ms: 1234.5, output: { text: "test result: FAILED", truncated: true } }),
  ]);
  expect(commands).toHaveLength(1);
  expect(commands[0]).toMatchObject({
    command: "cargo test",
    state: "failed",
    durationMs: 1234.5,
    output: "test result: FAILED",
    outputTruncated: true,
    sampled: false,
  });
});

it("marks in-flight text as the sample it is and never reads it as a return", () => {
  // A sampled tail is at most one snapshot a second and can miss everything between
  // two of them; calling it a result would turn "still going" into "finished".
  const commands = agentShell([
    event(1, "tool_call", { target: { kind: "command", text: "npm ci", truncated: false } }),
    event(2, "note", { partial: true, output: { text: "added 1 package", truncated: false } }),
  ]);
  expect(commands[0]).toMatchObject({ state: "unknown", sampled: true, output: "added 1 package" });
});

it("mirrors the agent's shell and nothing else it did", () => {
  const commands = agentShell([
    event(1, "tool_call", { name: "read", target: { kind: "path", text: "src/lib.rs", truncated: false } }),
    event(2, "tool_call", { name: "edit", call_id: "c2" }),
    event(3, "tool_call", { name: "bash", call_id: null }),
    event(4, "tool_call", { name: "powershell", call_id: "c4", target: { kind: "command", text: "Get-Date", truncated: false } }),
  ]);
  expect(commands.map((command) => command.command)).toEqual([null, "Get-Date"]);
});

it("keeps the newest commands rather than growing without bound", () => {
  const events = Array.from({ length: 30 }, (_, index) =>
    event(index, "tool_call", {
      call_id: `c${index}`,
      target: { kind: "command", text: `echo ${index}`, truncated: false },
    }),
  );
  const commands = agentShell(events, 20);
  expect(commands).toHaveLength(20);
  expect(commands.at(-1)?.command).toBe("echo 29");
});

it("never joins reused call IDs across nodes or sessions", () => {
  const commands = agentShell([
    { ...event(1, "tool_call", { target: { kind: "command", text: "one", truncated: false } }), session_id: "a" },
    { ...event(2, "tool_result", { outcome: "returned", output: { text: "other scope", truncated: false } }), node_run_id: 2, session_id: "b" },
  ]);
  expect(commands).toHaveLength(2);
  expect(commands[0]?.output).toBeNull();
});

it("keeps missing read intervals unknown rather than manufacturing silence", () => {
  let now = 0;
  const sampler = createRateSampler(() => now);
  now = 2000;
  expect(Array.from(sampler.series())).toEqual([null, null]);
  sampler.add("");
  now = 3000;
  expect(Array.from(sampler.series())).toEqual([null, null, 0]);
});

it("counts lines of output per second, whatever the bytes were", () => {
  // A line is a newline on a pty: multi-byte characters and escape sequences are output
  // like any other, and a rate that counted glyphs or bytes would say something else.
  let now = 10_000;
  const sampler = createRateSampler(() => now);
  sampler.add("\u001b[32m✓ ok\u001b[0m\n→ 🍋 next\n");
  now = 11_000;
  sampler.add("one\n");
  now = 12_000;
  expect(Array.from(sampler.series())).toEqual([2, 1]);

  // No polls established what happened during this interval.
  now = 15_000;
  expect(Array.from(sampler.series())).toEqual([2, 1, null, null, null]);
});
