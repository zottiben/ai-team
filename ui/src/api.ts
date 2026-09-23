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
  plan_slug?: string | null;
  workspace_path: string | null;
  blocked_reason?: string | null;
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
  worktree_path: string | null;
  branch: string | null;
  blocked_reason: string | null;
  session_id?: string | null;
  session_retired_at?: string | null;
  session_resetting_at?: string | null;
  context_tokens?: number | null;
  replyable?: boolean;
  recoverable?: boolean;
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
  node_run_id: number | null;
  kind: string;
  actor: string | null;
  summary: string;
  /** Full assistant or human prose, separated from the one-line evidence summary. */
  message: string | null;
  /** Provider-supplied reasoning summaries. Encrypted reasoning never crosses the API. */
  thinking: string[];
  at: string;
};

export function projects(): Promise<Project[]> {
  return api<Project[]>("/projects");
}

export type Notification = {
  id: number;
  project_id: number;
  workspace_path: string | null;
  run_id: number | null;
  node_run_id: number | null;
  kind: "plan_ready" | "completed" | "failed" | "input_required" | "follow_up";
  title: string;
  body: string;
  action_path: string | null;
  read_at: string | null;
  delivered_at: string | null;
  created_at: string;
};

export function notifications(limit = 60): Promise<Notification[]> {
  return api<Notification[]>(`/notifications?limit=${limit}`);
}

export function readNotification(id: number): Promise<Notification> {
  return post(`/notifications/${id}/read`, {});
}

export function runs(project?: number, workspace?: string | null): Promise<Run[]> {
  const params = new URLSearchParams();
  if (project !== undefined) params.set("project", String(project));
  if (workspace !== null && workspace !== undefined) params.set("workspace", workspace);
  const query = params.toString();
  return api<Run[]>(query === "" ? "/runs" : `/runs?${query}`);
}

export function run(id: number, workspace?: string | null): Promise<RunDetail> {
  const scope = workspace === null || workspace === undefined
    ? ""
    : `?workspace=${encodeURIComponent(workspace)}`;
  return api<RunDetail>(`/runs/${id}${scope}`);
}

export function runEvents(
  id: number,
  after?: number,
  workspace?: string | null,
): Promise<RunEvent[]> {
  const params = new URLSearchParams();
  if (after !== undefined) params.set("after", String(after));
  if (workspace !== null && workspace !== undefined) params.set("workspace", workspace);
  const query = params.toString();
  return api<RunEvent[]>(`/runs/${id}/events${query === "" ? "" : `?${query}`}`);
}

export type RunReply = { reached: "queued"; waiting: number };

export function replyToRun(
  runId: number,
  nodeId: number,
  message: string,
  workspace?: string | null,
): Promise<RunReply> {
  return post(`/runs/${runId}/nodes/${nodeId}/reply`, { message, workspace });
}

export function resumeRunNode(
  runId: number,
  nodeId: number,
  workspace: string,
): Promise<{ resumed: true; run_id: number; node_id: number }> {
  return post(`/runs/${runId}/nodes/${nodeId}/resume`, { workspace });
}

export type RunStartReceipt = {
  started: true;
  run_id?: number;
  continued?: true;
  preparing?: true;
};

export function approveRunPlan(runId: number): Promise<RunStartReceipt> {
  return post(`/runs/${runId}/approve-plan`, {});
}


export type BoardStatus =
  | "draft"
  | "ready"
  | "active"
  | "in_review"
  | "blocked"
  | "done"
  | "deferred";

export type BoardSlice = {
  id: number;
  plan_id: number;
  key: string;
  title: string;
  status: BoardStatus;
  ord: number;
  scope_md: string | null;
  demo_md: string | null;
  estimate_files: number | null;
  branch: string | null;
  base_branch: string | null;
  pr_url: string | null;
  worktree_path: string | null;
  claimed_by: string | null;
  claimed_at: string | null;
  blocked_reason: string | null;
  started_at: string | null;
  completed_at: string | null;
  rev: number;
  updated_at: string | null;
  /** ai-team's addition: the seat whose zone owns the declared paths. */
  owner: string | null;
  touches: string[];
  /** True only for ai-team's exact plan-approval hold, not an ordinary blocker. */
  approval_held?: boolean;
  delivery?: {
    run_id: number;
    node_run_id: number;
    branch: string;
    pushed_at: string | null;
    pr_url: string | null;
    merge_requested_at: string | null;
    delivery_claim: "push" | "pr" | "merge" | null;
    delivery_claimed_at: string | null;
    delivery_error: string | null;
    policy: DeliverySettings;
    remote: { pr_state: string; checks: string } | null;
  } | null;
};

export type BoardLogEntry = {
  id: number;
  plan_id: number;
  slice_key: string | null;
  at: string;
  actor: string | null;
  kind: string;
  branch: string | null;
  worktree_path: string | null;
  body: string;
};

export type BoardSliceDetail = { slice: BoardSlice; log: BoardLogEntry[] };

export type Board = {
  /** Null when this checkout has no plan yet - a normal state, not a failure. */
  plan: { plan: string; title: string; status: string; slice: string | null } | null;
  slices: BoardSlice[];
  next_step: string | null;
};

/** The statuses ai-planner recognises, in the order work moves through them. */
export const BOARD_COLUMNS: BoardStatus[] = [
  "draft",
  "ready",
  "active",
  "in_review",
  "blocked",
  "done",
  "deferred",
];

export const BOARD_STATUSES: { value: BoardStatus; label: string }[] = [
  { value: "draft", label: "Draft" },
  { value: "ready", label: "Ready" },
  { value: "active", label: "Active" },
  { value: "in_review", label: "In review" },
  { value: "blocked", label: "Blocked" },
  { value: "done", label: "Done" },
  { value: "deferred", label: "Deferred" },
];

export function board(project: string, workspace?: string | null): Promise<Board> {
  return api<Board>(`/board?${workspaceScope(project, workspace)}`);
}

export function moveSlice(
  key: string,
  body: { project: string; workspace?: string | null; status: BoardStatus; reason?: string },
): Promise<{ moved: string }> {
  return post(`/board/slices/${encodeURIComponent(key)}`, body);
}

export function boardSlice(
  project: string,
  key: string,
  workspace?: string | null,
): Promise<BoardSliceDetail> {
  return api<BoardSliceDetail>(
    `/board/slices/${encodeURIComponent(key)}?${workspaceScope(project, workspace)}`,
  );
}

export function claimBoardSlice(
  project: string,
  key: string,
  workspace?: string | null,
): Promise<unknown> {
  return post(`/board/slices/${encodeURIComponent(key)}/claim`, { project, workspace });
}

export function releaseBoardSlice(
  project: string,
  key: string,
  workspace?: string | null,
): Promise<unknown> {
  return post(`/board/slices/${encodeURIComponent(key)}/release`, { project, workspace });
}

export function editBoardSlice(
  project: string,
  key: string,
  prUrl: string,
  workspace?: string | null,
): Promise<unknown> {
  return request(`/board/slices/${encodeURIComponent(key)}`, "PATCH", {
    project,
    workspace,
    pr_url: prUrl,
  });
}

export function noteBoardSlice(
  project: string,
  key: string,
  body: string,
  workspace?: string | null,
): Promise<unknown> {
  return post(`/board/slices/${encodeURIComponent(key)}/notes`, { project, workspace, body });
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

export function reviews(
  project?: string | null,
  workspace?: string | null,
  openOnly = true,
): Promise<Review[]> {
  const params = new URLSearchParams({ open_only: String(openOnly) });
  if (project !== null && project !== undefined) params.set("project", project);
  if (workspace !== null && workspace !== undefined) params.set("workspace", workspace);
  return api<Review[]>(`/reviews?${params}`);
}

export function review(id: number, workspace?: string | null): Promise<ReviewDetail> {
  const suffix = workspace === null || workspace === undefined
    ? ""
    : `?workspace=${encodeURIComponent(workspace)}`;
  return api<ReviewDetail>(`/reviews/${id}${suffix}`);
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

export function submitReview(
  id: number,
  status: string,
  workspace?: string | null,
): Promise<Submitted> {
  return post(`/reviews/${id}/submit`, { status, workspace });
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

export function analytics(
  by: GroupBy,
  project: string | null,
  workspace?: string | null,
): Promise<AnalyticsRow[]> {
  const params = new URLSearchParams({ by });
  if (project !== null) params.set("project", project);
  if (workspace !== null && workspace !== undefined) params.set("workspace", workspace);
  return api<AnalyticsRow[]>(`/analytics?${params}`);
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

export type MapNode = {
  path: string;
  name: string;
  dir: boolean;
  depth: number;
  /** Files at or under this node; 1 for a file. What sizes a point on the map. */
  weight: number;
  /** The role of the seat whose zone claims this path, or null when nobody does. */
  owner: string | null;
};

export type MapEdge = { from: number; to: number };

export type MapZone = { role: string; name: string; zone: string; owns: number };

export type RepoMap = {
  root: string;
  nodes: MapNode[];
  edges: MapEdge[];
  zones: MapZone[];
  unowned: number;
  files: number;
  truncated: boolean;
};

/** The checkout, and which seat's zone claims each path (D14). */
export function repoMap(project: string, workspace?: string | null): Promise<RepoMap> {
  return api<RepoMap>(`/map?${workspaceScope(project, workspace)}`);
}

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
export type Where = {
  project: string;
  workspace?: string | null;
  node?: number | null;
};

function workspaceScope(project: string, workspace?: string | null): string {
  const params = new URLSearchParams({ project });
  if (workspace !== null && workspace !== undefined) params.set("workspace", workspace);
  return params.toString();
}

function scope(where: Where, extra: Record<string, string> = {}): string {
  const params = new URLSearchParams({ project: where.project, ...extra });
  if (where.workspace !== null && where.workspace !== undefined) {
    params.set("workspace", where.workspace);
  }
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
  /// The command that signs it in, when there is one. Null where signing in is not a
  /// command - a key in the environment, or a gateway on loopback with no account.
  sign_in: string | null;
};

export type ContextSetting = {
  source: string;
  allowed: boolean;
  /** Whether Pi already holds OAuth for this MCP server name. */
  oauth_connected: boolean;
  /** Optional manual-token fallback, not the normal connection path. */
  token_set: boolean;
  /// Where the token in force came from. A variable exported into the server's own
  /// process cannot be unset from here, so the field is read-only when it is "environment".
  held: "environment" | "keychain" | "absent";
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

/// Keep a context source's token, or clear it with an empty string.
///
/// The answer says whether there is now a token, never what it is - and neither does any
/// other route, which is why the field in the page is always blank when it loads.
export function setToken(source: string, token: string): Promise<{ token_set: boolean }> {
  return post("/settings/token", { source, token });
}

/// Start a provider's sign-in and get back the terminal session running it.
///
/// The command is not sent - the server looks it up from the provider, so the set of
/// things this can run is fixed rather than whatever the page asks for.
export function startSignIn(provider: string): Promise<{ id: number; command: string }> {
  return post("/settings/sign-in", { provider });
}

/** Start Pi's own browser OAuth flow for a read-only context source. */
export function startContextAuth(source: string): Promise<{ id: number; command: string }> {
  return post("/settings/context-auth", { source });
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

export type Candidate = {
  name: string;
  path: string;
  repo: boolean;
};

export type Listing = {
  path: string;
  parent: string | null;
  entries: Candidate[];
};

/// The directories inside one. An empty path means home, which is where a person's
/// checkouts are.
export function browse(path = ""): Promise<Listing> {
  return api<Listing>(`/browse?path=${encodeURIComponent(path)}`);
}

export type Worktree = {
  name: string;
  path: string;
  status: string;
  /// Who holds the lease - or, when awt has decided nobody does, why. A tree left
  /// leased across a reboot carries a sentence here rather than a name.
  lease_holder: string | null;
  processes: { pid: number; name: string }[];
  branch: string | null;
  /** The checkout registered on the project, rather than a linked task worktree. */
  main: boolean;
};

/// The worktrees `awt` is holding for a project, with the branch each has checked out.
export function worktrees(project: string): Promise<Worktree[]> {
  return api<Worktree[]>(`/worktrees?project=${encodeURIComponent(project)}`);
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

export type DeliveryPolicy = "manual" | "ask" | "auto";
export type DeliverySettings = {
  push: DeliveryPolicy;
  pr: DeliveryPolicy;
  merge: DeliveryPolicy;
};

export type Roster = {
  project: string | null;
  team: string | null;
  delivery?: DeliverySettings;
  seats: Seat[];
  available: string[];
};

export type ModelChoice = {
  /** ai-team's policy name, used when editing a seat. */
  provider: string;
  /** Pi's exact provider id, shown so subscription routing is never ambiguous. */
  runtime_provider: string;
  model: string;
  context: string;
  context_tokens?: number;
  max_output: string;
  thinking: boolean;
  images: boolean;
};

export type ModelCatalog = { models: ModelChoice[]; error: string | null };

export function models(): Promise<ModelCatalog> {
  return api<ModelCatalog>("/models");
}

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

export function detectOwnership(project: string): Promise<{ changed: number }> {
  return post("/roster/ownership/detect", { project });
}

export function resetRosterModels(project: string): Promise<{ changed: number }> {
  return post("/roster/models/reset", { project });
}

export function editDelivery(
  project: string,
  delivery: DeliverySettings,
): Promise<DeliverySettings> {
  return post("/roster/delivery", { project, ...delivery });
}

export type Doing =
  | "starting"
  | "working"
  | "parked"
  | "failed"
  | "idle"
  | "untouched"
  | "disabled";

export type MemberActivity = {
  kind: "step" | "tool_call" | "tool_result" | "cost" | "approval_request" | "approval_resolved" | "build" | "note" | "done" | "failed";
  summary: string;
  at: string;
};

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
  pushed_at?: string | null;
  pr_url?: string | null;
  merge_requested_at?: string | null;
  delivery_claim?: "push" | "pr" | "merge" | null;
  delivery_error?: string | null;
  delivery?: DeliverySettings;
  attempt: number;
  blocked_reason: string | null;
  last_said: string | null;
  activity?: MemberActivity | null;
  reachable: boolean;
  approval_run_id?: number | null;
  session_active?: boolean;
  session_resetting_at?: string | null;
  context_tokens?: number | null;
  turns: number;
  tokens_in: number;
  tokens_out: number;
};

export function crew(project: string, workspace?: string | null): Promise<Member[]> {
  return api<Member[]>(`/crew?${workspaceScope(project, workspace)}`);
}

export function deliverNode(
  runId: number,
  nodeId: number,
  action: "push" | "pr" | "merge",
  project: string,
  workspace: string,
): Promise<unknown> {
  return post(`/runs/${runId}/nodes/${nodeId}/deliver`, { action, project, workspace });
}

export function resetNodeSession(
  runId: number,
  nodeId: number,
  workspace: string,
): Promise<{ resetting: true; run_id: number; node_run_id: number }> {
  return post(`/runs/${runId}/nodes/${nodeId}/reset-session`, { workspace });
}

export type Reached =
  | { reached: "queued"; node_run_id: number; waiting: number }
  | { reached: "started" }
  | { reached: "coordinating" }
  | { reached: "continued"; run_id: number }
  | { reached: "refused"; because: string };

export function sayTo(
  agent: number,
  message: string,
  workspace?: string | null,
): Promise<Reached> {
  return post(`/crew/${agent}/say`, {
    message,
    ...(workspace === null || workspace === undefined ? {} : { workspace }),
  });
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
  workspace?: string | null;
  prompt?: string;
  replan?: boolean;
  plan_only?: boolean;
  approval_required?: boolean;
  action?: "approve_current";
}): Promise<RunStartReceipt> {
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
