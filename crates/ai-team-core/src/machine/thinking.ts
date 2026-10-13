// Loaded only for the no-prompt, no-session metadata probe. Pi's own helper owns
// the per-model map semantics; AI Team must not maintain another effort catalogue.
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { writeSync } from "node:fs";

export default function thinkingMetadata(pi: ExtensionAPI) {
  let reported = false;
  // No prompt is supplied, even if this extension cannot load. A startup extension
  // trying to start an ordinary agent turn must not turn discovery into paid work.
  pi.on("before_agent_start", () => process.exit(1));
  pi.on("session_start", (_event, ctx) => {
    const model = ctx.model;
    if (!model || reported) return;
    reported = true;
    writeSync(1, JSON.stringify({
      type: "ai_team_thinking",
      nonce: process.env.AI_TEAM_THINKING_PROBE,
      provider: model.provider,
      model: model.id,
      levels: getSupportedThinkingLevels(model),
    }) + "\n");
    // RPC stdin is closed by the host. That disposes the runtime without a prompt,
    // model call, saved session, or changing the operator's default thinking level.
  });
}
