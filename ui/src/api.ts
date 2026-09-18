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

export type Approval = {
  id: number;
  node_run_id: number | null;
  summary: string;
  payload: { request_id?: string; options?: { id: string; label: string }[] } | null;
};

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
  plan: { plan: string; title: string; status: string; slice: string | null };
  slices: BoardSlice[];
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

export async function post<T>(path: string, body: unknown): Promise<T> {
  const current = token();
  const response = await fetch(`/api${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(current ? { [TOKEN_HEADER]: current } : {}),
    },
    body: JSON.stringify(body),
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

export function approvals(runId: number): Promise<Approval[]> {
  return api<Approval[]>(`/runs/${runId}/approvals`);
}

/** Answer what a parked node asked. */
export function answer(
  runId: number,
  body: { node: number; request: string; chose: string },
): Promise<{ answered: boolean }> {
  return post(`/runs/${runId}/approvals`, body);
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
