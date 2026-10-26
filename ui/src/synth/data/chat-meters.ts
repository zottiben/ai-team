import type { ChatDisplay } from "./chat-display";
import { completedRates, count, measurementsByNode } from "./telemetry";

/** Selected chat's captured receipts, not an all-project spend or reconstructed history. */
export function chatMeters({ detail, events }: ChatDisplay, now: number) {
  const nodes = new Set(detail.turns.flatMap(turn => [turn.node.id, ...turn.members.map(member => member.node.id)]));
  const rows = [...measurementsByNode(events, nodes).values()].flat().sort((a, b) => a.id - b.id);
  const usage = rows.filter(row => row.telemetry.kind === "usage"), steps = usage.filter(row => row.telemetry.phase === "step");
  let tokens: number | null = steps.length ? 0 : null;
  for (const row of steps) if (row.telemetry.kind === "usage") {
    const input = count(row.telemetry.tokens.input), output = count(row.telemetry.tokens.output);
    tokens = tokens === null || input === null || output === null ? null : count(tokens + input + output);
  }
  const latest = usage.at(-1), stamp = rows.at(-1)?.observation?.at;
  const running = detail.state === "running" || detail.state === "stopping";
  const end = running ? now : stamp ? Date.parse(stamp) : now;
  return { tokens, steps: steps.length, node: latest?.node_run_id ?? null,
    context: latest?.telemetry.kind === "usage" ? count(latest.telemetry.tokens.context) : null,
    input: completedRates(rows, end, "input"), output: completedRates(rows, end), running };
}
