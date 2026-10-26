import type { RunEvent } from "../../../api";
import { trackerRows } from "../Session/Tracker/rows";

export type AgentCommand = {
  id: number;
  callId: string | null;
  nodeId: number | null;
  sessionId: string | null;
  correlation: string;
  command: string | null;
  commandTruncated: boolean;
  at: string;
  state: "returned" | "failed" | "unknown";
  durationMs: number | null;
  output: string | null;
  outputTruncated: boolean;
  sampled: boolean;
};

export const MIRROR_COMMANDS = 20;

/** Use the transcript's exact node/session/call correlation, including orphan evidence.
 * Missing returns and sampled output are never proof a command is still running. */
export function agentShell(events: readonly RunEvent[], limit = MIRROR_COMMANDS): AgentCommand[] {
  return trackerRows(events).flatMap((row): AgentCommand[] => {
    if (row.kind !== "tool" || row.lane !== "bash") return [];
    const event = row.start ?? row.result ?? row.update;
    if (!event) return [];
    const result = row.result?.tool;
    const target = row.start?.tool?.target ?? result?.target;
    const output = result?.output ?? row.update?.tool?.output;
    return [{
      id: row.id, callId: event.tool?.call_id ?? null, nodeId: event.node_run_id,
      sessionId: event.session_id ?? null, correlation: row.correlation,
      command: target?.kind === "command" ? target.text : null,
      commandTruncated: target?.truncated ?? false,
      at: event.at,
      state: result?.outcome === "returned" ? "returned" : result?.outcome === "failed" ? "failed" : "unknown",
      durationMs: row.duration?.source === "runtime" ? row.duration.milliseconds : null,
      output: output?.text ?? null, outputTruncated: output?.truncated ?? false,
      sampled: !result?.output && !!row.update?.tool?.output,
    }];
  }).slice(-Math.max(1, Math.min(MIRROR_COMMANDS, limit)));
}
