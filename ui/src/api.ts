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
