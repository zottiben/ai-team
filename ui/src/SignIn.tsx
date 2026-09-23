import { lazy, Suspense, useState } from "react";

import { startSignIn, type ProviderSetting } from "./api";

// Lazy, like every other use of the terminal. xterm is larger than the rest of the window
// put together, and importing it here would have put it in the eager chunk - which is a
// cost paid by everybody who opens ai-team so that a few people can sign in once. The
// bundle test caught exactly that, which is what it is for.
const Session = lazy(async () => ({ default: (await import("./Terminal")).Session }));

/**
 * Signing a provider in, from the page that noticed it was not.
 *
 * D17 says ai-team shows a command and never runs it, because `curl | sh` from a GUI
 * takes a decision that is not ai-team's to take. Signing in is a different thing and
 * D25 says so: `claude auth login` and `codex login` install nothing, touch only the
 * operator's own accounts, and cannot run unattended - they open a browser and wait for a
 * person. Installing a neighbour is still a command that gets copied.
 *
 * The command is shown either way, and it is the server that decides what it is: the
 * request names a provider, not a command line, so the set of things this can start is
 * fixed rather than whatever a page asks for.
 */
export function SignIn({
  provider,
  onDone,
}: {
  provider: ProviderSetting;
  onDone: () => void;
}) {
  const [session, setSession] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  if (provider.sign_in === null) return null;

  if (session !== null) {
    return (
      <div className="signin">
        <div className="card__row">
          <span className="faint mono">{provider.sign_in}</span>
          {/* Checking is manual on purpose. The flow ends in a browser, and a page that
              polled would either declare failure while somebody was still typing their
              password or keep asking a provider CLI a question every second. */}
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
        <Suspense fallback={<p className="empty">Opening a terminal…</p>}>
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
          void startSignIn(provider.provider)
            .then((started) => {
              setSession(started.id);
              setProblem(null);
            })
            .catch((error: unknown) => {
              setProblem(error instanceof Error ? error.message : String(error));
            });
        }}
      >
        Sign in
      </button>
      <code className="mono faint">{provider.sign_in}</code>
      {problem !== null && <span className="error">{problem}</span>}
    </span>
  );
}
