// Vite-only real-shell fixture. No operator storage, auth, SSE, PTY or API transport.
import { createRoot } from "react-dom/client";
import type { ChatDetail } from "../chat-api";
import type { Project, RunEvent } from "../api";
import "../styles.css";
import { fixtureActivity } from "./tracker-fixture";

class FixtureStorage implements Storage {
  constructor(private values = new Map<string, string>()) {}
  get length() { return this.values.size; }
  clear() { this.values.clear(); }
  getItem(key: string) { return this.values.get(key) ?? null; }
  key(index: number) { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string) { this.values.delete(key); }
  setItem(key: string, value: string) { this.values.set(key, String(value)); }
}
const params = new URLSearchParams(location.search);
const theme = params.get("theme") === "light" ? "light" : "dark";
const synth = params.get("synth") !== "false";
document.documentElement.dataset.theme = theme;
document.documentElement.dataset.synth = String(synth);
Object.defineProperty(window, "localStorage", { configurable: true, value: new FixtureStorage(new Map([
  ["ui.synth", String(synth)], ["ai-team.theme", theme], ["ai-team.last-chat", JSON.stringify({ project: "demo", chat: 1 })],
])) });
Object.defineProperty(window, "sessionStorage", { configurable: true, value: new FixtureStorage() });
const tickers = new Set<() => void>();
Object.defineProperty(window, "EventSource", { configurable: true, value: class {
  onmessage: (() => void) | null = null;
  onerror: (() => void) | null = null;
  tick = () => this.onmessage?.();
  constructor() { tickers.add(this.tick); }
  close() { tickers.delete(this.tick); }
} });
const project: Project = { id: 1, slug: "demo", name: "Demo project · fixture", kind: "repo", status: "active", open_runs: 0 };
const chat: ChatDetail = { id: 1, project_id: 1, title: "Shape the workspace", mode: "single", workspace_path: "/fixture/demo", provider: "openai", model: "fixture-model", reasoning: "high",
  active_node_id: null, live_text: "", stop_requested: false, archived: false, rev: 0, created_at: "2026-01-01", updated_at: "2026-01-01", turns: [], followups: [], team_builds: [], state: "idle", can_resume: false, orphan_running: false };
const events: RunEvent[] = [{ id: 1, node_run_id: 1, actor: "human", kind: "note", summary: "Fixture message", message: "Keep this conversation and its unsent draft while exploring the workspace.", thinking: [], at: "2026-01-01" },
  { id: 2, node_run_id: 1, actor: "assistant", kind: "note", summary: "Fixture response", message: "This is an isolated **layout fixture**. Tabs and tools share the real application controller. Network access and all backend mutations are disabled.", thinking: [], at: "2026-01-01" }];
const activityCount = Math.max(0, Math.min(5_000, Number(params.get("activity")) || 0));
const activityMode = params.get("activity-mode") === "reads" ? "reads" : params.get("activity-mode") === "thinking" ? "thinking" : "tools";
events.push(...fixtureActivity(activityCount, 2, activityMode));
export function appendActivity() {
  events.push(...fixtureActivity(1, events.at(-1)?.id ?? 0, activityMode));
  chat.rev += 1;
  tickers.forEach(tick => tick());
}
export function appendShellPhase(phase: "start" | "partial" | "end", text: string) {
  const id = (events.at(-1)?.id ?? 0) + 1;
  events.push({ id, node_run_id: 1, session_id: "fixture-session", actor: "assistant", message: null, thinking: [], at: "2026-01-01T00:00:00Z",
    kind: phase === "partial" ? "note" : phase === "start" ? "tool_call" : "tool_result", summary: "Synthetic sampled shell fixture",
    tool: { name: "bash", call_id: "fixture-streamed-shell", parent_call_id: null, partial: phase === "partial",
      target: phase === "start" ? { kind: "command", text: "Synthetic shell fixture · no process", truncated: false } : null,
      outcome: phase === "end" ? "returned" : null, duration_ms: null, output: phase === "start" ? null : { text, truncated: true } } });
  chat.rev += 1;
  tickers.forEach(tick => tick());
  return id;
}
window.fetch = async (input, init) => {
  const request = input instanceof Request ? input : null;
  const url = new URL(request?.url ?? String(input), location.href), method = init?.method ?? request?.method ?? "GET";
  const json = (data: unknown, status = 200) => new Response(JSON.stringify(data), { status, headers: { "Content-Type": "application/json" } });
  if (method !== "GET") return json({ error: "Fixture only: backend mutations and processes are disabled." }, 403);
  switch (url.pathname) {
    case "/api/projects": return json([project]);
    case "/api/chats": return json([chat]);
    case "/api/chats/1": return json(chat);
    case "/api/chats/1/events": return json(events.filter(event => event.id > Number(url.searchParams.get("after") ?? 0)).slice(0, 500));
    case "/api/chats/1/workspace": return json({ requests: [] });
    case "/api/chats/1/plan": return json({ chat_id: 1, project_id: 1, revision: 0, bundle: null });
    case "/api/chat-workspace-setups": return json([]);
    case "/api/doctor": return json({ needs_setup: false });
    case "/api/notifications": return json({ items: [], unread: 0 });
    case "/api/models/thinking": return json({ provider: "openai", model: "fixture-model", levels: ["none", "low", "medium", "high"], fallback_reason: null });
    case "/api/update": return json({ current: "0.0.0-fixture", latest: "0.0.1-fixture", method: "release", can_update: false, update_available: true, blocked: "Installation is disabled in this fixture.", targets: [], approval: null, state: "idle", checked_at: null, release_url: "about:blank" });
    default: return json({ error: `This fixture has no data for ${url.pathname}. No network request was made.` }, 404);
  }
};
const { default: App } = await import("../App");
const root = document.getElementById("root");
if (!root) throw new Error("Missing fixture root");
createRoot(root).render(<App />);
