import type { CSSProperties } from "react";

const tokens = {
  think: "--lane-think", read: "--lane-read", edit: "--lane-edit", bash: "--lane-bash",
  mcp: "--lane-mcp", guard: "--lane-guard", live: "--lane-live", assistant: "--lane-assistant",
  ticket: "--sig-ticket", spec: "--sig-spec", plan: "--sig-plan", diff: "--sig-diff",
  verdict: "--sig-verdict", fix: "--sig-fix", result: "--sig-result", activity: "--sig-activity",
  working: "--st-working", done: "--st-done", attention: "--st-attention", info: "--st-info", idle: "--st-idle",
  "slice-draft": "--slice-draft", "slice-ready": "--slice-ready", "slice-active": "--slice-active", "slice-review": "--slice-review",
  "slice-blocked": "--slice-blocked", "slice-done": "--slice-done", "slice-deferred": "--slice-deferred",
} as const;
export type Signal = keyof typeof tokens;
export function signalStyle(signal: Signal): CSSProperties & { "--synth-signal": string } {
  return { "--synth-signal": `var(${tokens[signal]})` };
}
