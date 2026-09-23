import { lazy, Suspense, useState } from "react";

import { startContextAuth, type ContextSetting } from "./api";

// xterm is larger than the rest of the eager bundle. OAuth is a once-per-source setup
// flow, so it pays that cost only after somebody presses Connect, like provider sign-in.
const Session = lazy(async () => ({ default: (await import("./Terminal")).Session }));

/**
 * Connect one MCP context source through Pi's own browser OAuth.
 *
 * ai-team does not own an OAuth client, callback server or refresh token. The button
 * starts interactive Pi with the exact read-only MCP definition a seat receives and Pi's
 * `/mcp-auth <server>` command does the rest. That makes an OAuth already completed in
 * the operator's Pi the same credential ai-team sees - one account, not a second login.
 */
export function ContextConnect({
  source,
  onDone,
}: {
  source: ContextSetting;
  onDone: () => void;
}) {
  const [session, setSession] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  if (session !== null) {
    return (
      <div className="signin">
        <div className="card__row">
          <span className="faint mono">/mcp-auth {source.source}</span>
          {/* The flow ends in a browser. Polling would call it failed while somebody was
              still choosing an account, so the operator says when to check Pi's store. */}
          <button
            type="button"
            className="button"
            onClick={() => {
              setSession(null);
              onDone();
            }}
          >
            Done - check again
          </button>
        </div>
        <Suspense fallback={<p className="empty">Opening Pi…</p>}>
          <Session id={session} />
        </Suspense>
      </div>
    );
  }

  return (
    <span className="signin__offer">
      <button
        type="button"
        className="button"
        onClick={() => {
          void startContextAuth(source.source)
            .then((started) => {
              setSession(started.id);
              setProblem(null);
            })
            .catch((error: unknown) => {
              setProblem(error instanceof Error ? error.message : String(error));
            });
        }}
      >
        {source.oauth_connected ? "Reconnect" : "Connect in browser"}
      </button>
      <code className="mono faint">/mcp-auth {source.source}</code>
      {problem !== null && <span className="error">{problem}</span>}
    </span>
  );
}
