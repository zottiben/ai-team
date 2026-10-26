import type { AnalyticsRow, DoctorReport, ModelCatalog, Project, Roster, Settings } from "../api";
import type { ChatDetail } from "../chat-api";
import type { ChatToday } from "../chat-today-api";
import type { PlanLibrary } from "../plan-library-api";

export const fixtureModels: ModelCatalog = { error: null, models: [
  { provider: "openai", runtime_provider: "openai-codex", model: "fixture-model", thinking: true, images: true, context: "200k", max_output: "32k" },
  { provider: "local", runtime_provider: "llama.cpp", model: "fixture-small", thinking: false, images: false, context: "32k", max_output: "8k" },
] };

/** Read-only synthetic global pages. No network, provider, process or machine state. */
export function globalFixture(project: Project, chat: ChatDetail) {
  const doctor: DoctorReport = { version: "0.0.0-fixture", needs_setup: false, can_run: true, severity: "fine", checks: [
    { id: "planning", label: "Built-in planning", severity: "fine", detail: "Synthetic chat-owned plans", fix: { by: "none" } },
    { id: "toolbox", label: "Built-in toolbox", severity: "fine", detail: "Synthetic setup catalogue", fix: { by: "none" } },
    { id: "project.demo", label: project.name, severity: "fine", detail: "/fixture/demo", fix: { by: "none" } },
  ] };
  const settings: Settings = { profile_path: "/fixture/config/machine.toml", fallback: ["openai", "local", "claude", "zai"],
    providers: ["openai", "local", "claude", "zai"].map(provider => ({ provider, label: provider, allowed: provider === "openai", reachable: provider === "openai", detail: "Synthetic subscription readiness", how: "Disabled in this fixture", sign_in: null })),
    context: [{ source: "figma", allowed: true, oauth_connected: true, token_set: false, held: "absent", token_env: "FIXTURE_ONLY" }],
  };
  const roster: Roster = { project: null, team: null, available: ["openai"], seats: ["orchestrator", "planner", "backend", "frontend", "verifier", "reviewer"].map((role, index) => ({ id: -index - 1, role, name: role.charAt(0).toUpperCase() + role.slice(1), purpose: "Synthetic role description. Each seat retains its own exact model, instructions and authority.", provider: "openai", model: "fixture-model", effective_provider: "openai", effective_model: "fixture-model", fallback_reason: null, reasoning: "high", zone: index === 2 ? "crates/**" : index === 3 ? "ui/**" : "", read_only: ![2,3].includes(index), enabled: true })) };
  const library: PlanLibrary = { projects: [{ ...project, plans: 1 }], detached: [], destinations: [], entries: [{ plan_id: 1, slug: "fixture-plan", title: "Synthetic retained planning", status: "ready", summary: "Synthetic global board", project_id: 1, project_slug: project.slug, project_name: project.name, chat_id: 1, chat_title: chat.title, chat_archived: false, slices: 100, done: 14, open_questions: 1, updated_at: "2026-01-01", last_activity: null, imported: null }] };
  const today: ChatToday = { chats: 1, needs_attention: 1, working: 0, drafts: 0, entries: [{ chat_id: 1, project_slug: project.slug, project_name: project.name, title: "Synthetic long conversation title — review the exact chat before continuing", workspace_path: chat.workspace_path, updated_at: "2026-01-01", state: "questions", detail: "One explicit decision is waiting", panel: "overview", needs_attention: true, working: false, drafts: 0, questions: 1 }] };
  const analytics: AnalyticsRow[] = ["assistant", "backend", "planner"].map((group, i) => ({ group, attempts: 10, accepted: 8, rejected: 2, changes_accepted: 4, tokens_in: 10000, tokens_out: 1000, cache_read: 20000, cache_write: 500, seconds: 60, cycle_seconds: 90, gates_run: 4, gates_passed: 3, accepted_rate: .8, rework: i === 0 ? null : 1.25, total_input: 30500, input_per_accepted: 7625, cache_hit_rate: .65, yield_per_k: 32.8, gate_pass_rate: .75, cycle_time: 90 }));
  return (url: URL): unknown => {
    switch (url.pathname) {
      case "/api/doctor": return doctor;
      case "/api/settings": return settings;
      case "/api/mcp/servers": return [];
      case "/api/roster": return roster;
      case "/api/plan-library": return library;
      case "/api/chat-today": return today;
      case "/api/analytics": return analytics;
      case "/api/reminders": return [{ id: 1, project_id: 1, chat_id: null, kind: "idea", title: "Synthetic idea for the next session", body: "No scheduled execution in this fixture", prompt: null, due_at: null, recur: null, status: "pending", last_fired_at: null }];
      case "/api/chat-schedules": return [];
      case "/api/notifications/inbox": return { unread: 2, items: [2,1].map(id => ({ id, project_id: 1, chat_id: 1, run_id: null, node_run_id: null, workspace_path: chat.workspace_path, kind: "turn_complete", title: `Synthetic completion ${id} — conversation with a long title`, body: "Ready for the next message. Completion is not a verification verdict.", created_at: "2026-01-01", read_at: null, cleared_at: null })) };
      default: return undefined;
    }
  };
}
