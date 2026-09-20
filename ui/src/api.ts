// Talking to the binary that served this page.
//
// The server mints a session token and puts it in the URL that opened the tab, because
// that is the only channel a fresh tab has. We take it out of the address bar
// immediately: a URL that carries a credential gets bookmarked, pasted into a chat, and
// restored by the browser weeks later.

const TOKEN_HEADER = "x-ai-team-token";
const TOKEN_QUERY = "token";
const STORAGE_KEY = "ai-team.token";

/// Read the token from the URL if it is there, remember it for this tab, and scrub the
/// query. Safe to call more than once - a reload has no token in the URL and falls
/// through to what was stored.
export function captureToken(location: Location, history: History): string | null {
  const url = new URL(location.href);
  const fromUrl = url.searchParams.get(TOKEN_QUERY);

  if (fromUrl) {
    sessionStorage.setItem(STORAGE_KEY, fromUrl);
    url.searchParams.delete(TOKEN_QUERY);
    history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
    return fromUrl;
  }

  // sessionStorage, not localStorage: the token dies with the process that minted it,
  // so keeping it past the tab would only ever produce a confusing 401.
  return sessionStorage.getItem(STORAGE_KEY);
}

export function token(): string | null {
  return sessionStorage.getItem(STORAGE_KEY);
}

export async function api<T>(path: string): Promise<T> {
  const current = token();
  const response = await fetch(`/api${path}`, {
    headers: current ? { [TOKEN_HEADER]: current } : {},
  });
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(body?.error ?? `${path} failed with ${response.status}`);
  }
  return (await response.json()) as T;
}

export type Health = {
  version: string;
  bundle_embedded: boolean;
  bundle_files: number;
};

export function health(): Promise<Health> {
  return api<Health>("/health");
}

export type Project = {
  id: number;
  slug: string;
  name: string;
  kind: string;
  status: string;
  open_runs: number;
};

export type Run = {
  id: number;
  project_id: number;
  prompt: string;
  status: string;
  trigger: string;
  created_at: string;
  started_at: string | null;
  ended_at: string | null;
};

export type NodeRun = {
  id: number;
  role: string;
  provider: string;
  model: string;
  status: string;
  attempt: number;
  slice_key: string | null;
  branch: string | null;
  blocked_reason: string | null;
};

export type Usage = {
  tokens_in: number;
  tokens_out: number;
  cache_read: number;
  cache_write: number;
};

export type RunDetail = Run & { nodes: NodeRun[]; usage: Usage };

export type RunEvent = {
  id: number;
  kind: string;
  actor: string | null;
  summary: string;
  at: string;
};

export function projects(): Promise<Project[]> {
  return api<Project[]>("/projects");
}

export function runs(project?: number): Promise<Run[]> {
  return api<Run[]>(project === undefined ? "/runs" : `/runs?project=${project}`);
}

export function run(id: number): Promise<RunDetail> {
  return api<RunDetail>(`/runs/${id}`);
}

export function runEvents(id: number, after?: number): Promise<RunEvent[]> {
  return api<RunEvent[]>(`/runs/${id}/events${after === undefined ? "" : `?after=${after}`}`);
}


export type BoardSlice = {
  key: string;
  title: string;
  status: string;
  ord: number;
  scope_md: string | null;
  demo_md: string | null;
  claimed_by: string | null;
  owner: string | null;
  touches: string[];
};

export type Board = {
  /** Null when this checkout has no plan yet - a normal state, not a failure. */
  plan: { plan: string; title: string; status: string; slice: string | null } | null;
  slices: BoardSlice[];
  next_step: string | null;
};

/** The statuses ai-planner recognises, in the order work moves through them. */
export const BOARD_COLUMNS = [
  "draft",
  "ready",
  "active",
  "in_review",
  "blocked",
  "done",
  "deferred",
] as const;

export function board(project: string): Promise<Board> {
  return api<Board>(`/board?project=${encodeURIComponent(project)}`);
}

export function moveSlice(
  key: string,
  body: { project: string; status: string; reason?: string },
): Promise<{ moved: string }> {
  return post(`/board/slices/${encodeURIComponent(key)}`, body);
}

export type TodayItem = {
  urgency: "blocking" | "overdue" | "failed" | "review" | "question" | "due" | "in_flight";
  kind: string;
  title: string;
  detail: string | null;
  project: string | null;
  run_id: number | null;
  since: string | null;
};

export function today(): Promise<TodayItem[]> {
  return api<TodayItem[]>("/today");
}

export type DiffLine = {
  kind: "context" | "added" | "removed";
  old: number | null;
  new: number | null;
  text: string;
};

export type FileDiff = {
  path: string;
  old_path: string | null;
  status: "added" | "modified" | "removed" | "renamed";
  binary: boolean;
  hunks: { header: string; old_start: number; new_start: number; lines: DiffLine[] }[];
  additions: number;
  deletions: number;
};

export type Comment = {
  id: number;
  review_id: number;
  parent_id: number | null;
  file_path: string | null;
  side: "old" | "new" | null;
  line_start: number | null;
  line_end: number | null;
  author: string;
  body: string;
  status: "open" | "resolved" | "outdated";
  created_at: string;
};

export type Review = {
  id: number;
  project_id: number;
  run_id: number | null;
  node_run_id: number | null;
  title: string;
  status: string;
  branch: string | null;
  submitted_at: string | null;
};

export type ReviewDetail = Review & {
  files: FileDiff[];
  comments: Comment[];
  steerable: boolean;
};

export type Submitted =
  | {
      outcome: "steered";
      node_run_id: number;
      comments: number;
      /** Whether the orchestrator was told directly as well as through the plan. */
      told_orchestrator: boolean;
    }
  | { outcome: "planned"; slice_key: string; comments: number }
  | { outcome: "accepted" };

export function reviews(openOnly = true): Promise<Review[]> {
  return api<Review[]>(`/reviews?open_only=${openOnly}`);
}

export function review(id: number): Promise<ReviewDetail> {
  return api<ReviewDetail>(`/reviews/${id}`);
}

export function addComment(
  id: number,
  body: {
    body: string;
    file_path?: string;
    side?: "old" | "new";
    line_start?: number;
    line_end?: number;
    parent_id?: number;
  },
): Promise<Comment> {
  return post(`/reviews/${id}/comments`, body);
}

export function resolveComment(id: number): Promise<Comment> {
  return post(`/comments/${id}/resolve`, {});
}

export function submitReview(id: number, status: string): Promise<Submitted> {
  return post(`/reviews/${id}/submit`, { status });
}

export type AnalyticsRow = {
  group: string;
  attempts: number;
  accepted: number;
  rejected: number;
  slices_accepted: number;
  tokens_in: number;
  tokens_out: number;
  cache_read: number;
  cache_write: number;
  seconds: number;
  cycle_seconds: number;
  gates_run: number;
  gates_passed: number;
  accepted_rate: number | null;
  rework: number | null;
  total_input: number;
  input_per_accepted: number | null;
  cache_hit_rate: number | null;
  yield_per_k: number | null;
  gate_pass_rate: number | null;
  cycle_time: number | null;
};

export type GroupBy = "agent" | "model" | "team" | "project";

export function analytics(by: GroupBy, project: string | null): Promise<AnalyticsRow[]> {
  const scope = project === null ? "" : `&project=${encodeURIComponent(project)}`;
  return api<AnalyticsRow[]>(`/analytics?by=${by}${scope}`);
}

export type Reminder = {
  id: number;
  project_id: number | null;
  kind: "reminder" | "idea" | "scheduled_run";
  title: string;
  body: string;
  prompt: string | null;
  due_at: string | null;
  recur: string | null;
  status: "pending" | "fired" | "done" | "cancelled";
  last_fired_at: string | null;
};

export function reminders(project: string | null): Promise<Reminder[]> {
  const scope = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return api<Reminder[]>(`/reminders${scope}`);
}

export function addReminder(body: {
  title: string;
  kind?: Reminder["kind"];
  project?: string;
  due_at?: string;
  recur?: string;
  prompt?: string;
}): Promise<Reminder> {
  return post("/reminders", body);
}

export function cancelReminder(id: number): Promise<Reminder> {
  return request(`/reminders/${id}`, "DELETE");
}

export type TreeEntry = { path: string; name: string; dir: boolean };

export type FileBody = { path: string; text: string; editable: boolean };

export type Hit = {
  path: string;
  language: string | null;
  summary: string | null;
  symbol: string | null;
  score: number;
  start_line: number;
  end_line: number;
  snippet: string;
};

/** Every editor call is scoped to a checkout: a project, or a node's leased worktree. */
export type Where = { project: string; node?: number | null };

function scope(where: Where, extra: Record<string, string> = {}): string {
  const params = new URLSearchParams({ project: where.project, ...extra });
  if (where.node !== null && where.node !== undefined) params.set("node", String(where.node));
  return params.toString();
}

export function tree(where: Where, path: string): Promise<TreeEntry[]> {
  return api<TreeEntry[]>(`/tree?${scope(where, { path })}`);
}

export function readFile(where: Where, path: string): Promise<FileBody> {
  return api<FileBody>(`/file?${scope(where, { path })}`);
}

export function saveFile(where: Where, path: string, text: string): Promise<{ saved: string }> {
  return post("/file", { ...where, path, text });
}

export function search(where: Where, q: string): Promise<Hit[]> {
  return api<Hit[]>(`/search?${scope(where, { q })}`);
}

export type LspRange = {
  start: { line: number; character: number };
  end: { line: number; character: number };
};

export type LspDiagnostic = {
  range: LspRange;
  severity: number | null;
  source: string | null;
  message: string;
};

export type Diagnostics = {
  /** False when no server handles this language - the gutter draws nothing. */
  analysed: boolean;
  /** Null until the server has published at all, which is not the same as clean. */
  diagnostics: LspDiagnostic[] | null;
};

export type RenameEdit = { path: string; range: LspRange; new_text: string };

/** Every language request carries the buffer, so a mistake is seen before it is saved. */
type LspBody = Where & { path: string; text: string; line?: number; character?: number };

export function lspDiagnostics(body: LspBody): Promise<Diagnostics> {
  return post("/lsp/diagnostics", body);
}

export function lspHover(body: LspBody): Promise<{ text: string } | null> {
  return post("/lsp/hover", body);
}

export function lspDefinition(body: LspBody): Promise<{ path: string; range: LspRange }[]> {
  return post("/lsp/definition", body);
}

export function lspCompletion(body: LspBody): Promise<string[]> {
  return post("/lsp/completion", body);
}

export function lspRename(body: LspBody & { new_name: string }): Promise<RenameEdit[]> {
  return post("/lsp/rename", body);
}

export type TerminalSession = { id: number; worktree: string; done: boolean; status: number | null };

export type TerminalChunk = { text: string; cursor: number; done: boolean; status: number | null };

export function terminals(where: Where): Promise<TerminalSession[]> {
  return api<TerminalSession[]>(`/terminals?${scope(where)}`);
}

export function openTerminal(where: Where): Promise<{ id: number }> {
  return post("/terminals", where);
}

export function readTerminal(id: number, cursor: number): Promise<TerminalChunk> {
  return api<TerminalChunk>(`/terminals/${id}?cursor=${cursor}`);
}

export function writeTerminal(id: number, text: string): Promise<{ sent: number }> {
  return post(`/terminals/${id}`, { text });
}

export function resizeTerminal(id: number, rows: number, cols: number): Promise<unknown> {
  return post(`/terminals/${id}/resize`, { rows, cols });
}

export function closeTerminal(id: number): Promise<unknown> {
  return post(`/terminals/${id}/close`, {});
}

export type Scm = {
  branch: string | null;
  branches: string[];
  unstaged: FileDiff[];
  staged: FileDiff[];
  untracked: string[];
};

export function scm(where: Where): Promise<Scm> {
  return api<Scm>(`/scm?${scope(where)}`);
}

export function scmStage(
  where: Where,
  body: { path: string; hunk?: number; unstage?: boolean },
): Promise<unknown> {
  return post("/scm/stage", { ...where, ...body });
}

export function scmCommit(where: Where, message: string): Promise<{ sha: string }> {
  return post("/scm/commit", { ...where, message });
}

export function scmBranch(where: Where, branch: string): Promise<unknown> {
  return post("/scm/branch", { ...where, branch });
}

export function scmPush(where: Where): Promise<{ pushed: string }> {
  return post("/scm/push", where);
}

export type Available = {
  current: string;
  latest: string | null;
  method: "release" | "source" | "unknown";
  can_update: boolean;
  blocked: string | null;
};

export function updateCheck(): Promise<Available> {
  return api<Available>("/update");
}

export function updateApply(): Promise<{ version: string; restart_required: boolean }> {
  return post("/update", {});
}

export type Check = {
  id: string;
  label: string;
  severity: "blocking" | "degraded" | "fine";
  detail: string;
  fix:
    | { by: "itself"; action: string; describe: string }
    | { by: "command"; run: string; why: string }
    | { by: "human"; what: string }
    | { by: "none" };
};

export type DoctorReport = {
  version: string;
  checks: Check[];
  severity: "blocking" | "degraded" | "fine";
  can_run: boolean;
  needs_setup: boolean;
};

export function doctor(): Promise<DoctorReport> {
  return api<DoctorReport>("/doctor");
}

/** Only the repairs ai-team owns are expressible here (D17). */
export function doctorFix(action: string): Promise<{ done: string }> {
  return post("/doctor/fix", { action });
}

export type ProviderSetting = {
  provider: string;
  label: string;
  allowed: boolean;
  reachable: boolean;
  detail: string;
  how: string;
};

export type ContextSetting = {
  source: string;
  allowed: boolean;
  token_set: boolean;
  token_env: string;
};

export type Settings = {
  profile_path: string;
  providers: ProviderSetting[];
  fallback: string[];
  context: ContextSetting[];
};

export function settings(): Promise<Settings> {
  return api<Settings>("/settings");
}

export function setProvider(provider: string, allowed: boolean): Promise<unknown> {
  return post("/settings/provider", { provider, allowed });
}

export function setContext(source: string, allowed: boolean): Promise<unknown> {
  return post("/settings/context", { source, allowed });
}

export function setFallback(order: string[]): Promise<unknown> {
  return post("/settings/fallback", { order });
}

export type Registered = {
  project: { id: number; slug: string; name: string; kind: string };
  repo_path: string | null;
  created: boolean;
  seeded_team: boolean;
  roster: [string, string, string][];
};

export function registerProject(body: {
  path: string;
  name?: string;
  kind?: string;
}): Promise<Registered> {
  return post("/projects", body);
}

export function attachRepo(id: number, path: string): Promise<{ attached: string }> {
  return post(`/projects/${id}/repos`, { path });
}

export type Seat = {
  id: number;
  role: string;
  name: string;
  purpose: string;
  provider: string;
  model: string;
  effective_provider: string;
  effective_model: string;
  fallback_reason: string | null;
  reasoning: string;
  zone: string;
  read_only: boolean;
  enabled: boolean;
};

export type Roster = {
  project: string | null;
  team: string | null;
  seats: Seat[];
  available: string[];
};

/** Omit the project to ask what a new one would get. */
export function roster(project: string | null): Promise<Roster> {
  const scope = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return api<Roster>(`/roster${scope}`);
}

export function editSeat(
  id: number,
  change: { provider?: string; model?: string; zone?: string; enabled?: boolean },
): Promise<unknown> {
  return post(`/roster/${id}`, change);
}

export type Doing =
  | "starting"
  | "working"
  | "parked"
  | "failed"
  | "idle"
  | "untouched"
  | "disabled";

export type Member = {
  agent_id: number;
  role: string;
  name: string;
  provider: string;
  model: string;
  read_only: boolean;
  zone: string;
  doing: Doing;
  node_run_id: number | null;
  run_id: number | null;
  slice_key: string | null;
  branch: string | null;
  attempt: number;
  blocked_reason: string | null;
  last_said: string | null;
  reachable: boolean;
  turns: number;
  tokens_in: number;
  tokens_out: number;
};

export function crew(project: string): Promise<Member[]> {
  return api<Member[]>(`/crew?project=${encodeURIComponent(project)}`);
}

export type Reached =
  | { reached: "queued"; node_run_id: number; waiting: number }
  | { reached: "started" }
  | { reached: "refused"; because: string };

export function sayTo(agent: number, message: string): Promise<Reached> {
  return post(`/crew/${agent}/say`, { message });
}

export function post<T>(path: string, body: unknown): Promise<T> {
  return request(path, "POST", body);
}

/** One place that writes, so the token and the error shape are decided once. */
export async function request<T>(path: string, method: string, body?: unknown): Promise<T> {
  const current = token();
  const response = await fetch(`/api${path}`, {
    method,
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(current ? { [TOKEN_HEADER]: current } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(problem?.error ?? `${path} failed with ${response.status}`);
  }
  return (await response.json()) as T;
}

/** Start a run. Returns once it is under way, not once it has finished. */
export function startRun(request: {
  project: string;
  prompt?: string;
  replan?: boolean;
}): Promise<{ started: boolean }> {
  return post("/runs", request);
}

/**
 * Subscribe to "something changed".
 *
 * The token rides in the query because EventSource cannot set a header. That is safe
 * here in a way it would not be on a public URL: the server is on loopback and the token
 * is already in this tab's storage.
 *
 * Deliberately thin - a tick says only that the database moved, and the caller re-reads
 * whichever view it is showing. Streaming rows would mean the server knowing what every
 * surface renders.
 */
export function subscribe(onTick: () => void): () => void {
  const current = token();
  const source = new EventSource(`/api/events?${TOKEN_QUERY}=${encodeURIComponent(current ?? "")}`);
  source.onmessage = () => onTick();
  // EventSource reconnects on its own; this is only so a dead server does not look like
  // a live one that has gone quiet.
  source.onerror = () => {};
  return () => source.close();
}
