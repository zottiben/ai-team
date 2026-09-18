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
