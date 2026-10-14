// Synthetic activity for Vite-only layout checks. Never used as runtime telemetry.
import type { RunEvent } from "../api";

export function fixtureActivity(count: number, after = 2, mode: "tools" | "reads" | "thinking" = "tools"): RunEvent[] {
  const events: RunEvent[] = [];
  let id = after;
  for (let index = 0; index < count; index++) {
    const call = `fixture-${id}`;
    const base = { node_run_id: 1, session_id: "fixture-session", actor: "assistant", message: null, thinking: [], at: "2026-01-01T00:00:00Z" };
    if (mode === "thinking") {
      events.push({ ...base, id: ++id, kind: "cost", summary: "Fixture summary", thinking: [`Thinking fixture ${index}\n${"Variable-height retained summary.\n".repeat(1 + index % 3)}`] });
      continue;
    }
    const name = mode === "reads" ? "read" : "bash";
    events.push({ ...base, id: ++id, kind: "tool_call", summary: name, tool: { name, call_id: call, parent_call_id: null,
      target: { kind: mode === "reads" ? "path" : "command", text: mode === "reads" ? `fixture/file-${index}.ts` : `printf 'Fixture command ${index}'`, truncated: false }, outcome: null, duration_ms: null, output: null } });
    events.push({ ...base, id: ++id, kind: "tool_result", summary: `${name} returned`, tool: { name, call_id: call, parent_call_id: null, target: null,
      outcome: index % 31 === 0 ? "failed" : "returned", duration_ms: index % 30 + .5,
      output: mode === "reads" ? null : { text: `Fixture output ${index}\n${"A deliberately variable-height line of retained output.\n".repeat(8 + index % 7)}`, truncated: false } } });
    if (mode === "tools" && index % 13 === 0) events.push({ ...base, id: ++id, kind: "note", summary: "Fixture prose", message: `**Fixture prose ${index}**\n\n${"Retained variable-height Markdown.\n\n".repeat(1 + index % 3)}` });
  }
  return events;
}
