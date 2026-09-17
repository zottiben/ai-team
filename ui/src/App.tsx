import { useEffect, useState } from "react";
import { health, type Health } from "./api";

// The app shell - sidebar, console, right dock - is M3-S11. Until then this proves the
// one thing M0-S1 is responsible for: the bundle in the binary is being served, and the
// page behind it can reach the API with the token it was handed.
export default function App() {
  const [state, setState] = useState<Health | Error | null>(null);

  useEffect(() => {
    let live = true;
    health()
      .then((value) => live && setState(value))
      .catch((error: unknown) => {
        if (live) setState(error instanceof Error ? error : new Error(String(error)));
      });
    return () => {
      live = false;
    };
  }, []);

  return (
    <main>
      <h1>ai-team</h1>
      <p>Nothing is wired up yet. This is the workspace skeleton.</p>
      {state === null && <p>Checking the server…</p>}
      {state instanceof Error && <p className="error">Cannot reach the server: {state.message}</p>}
      {state !== null && !(state instanceof Error) && (
        <dl>
          <dt>version</dt>
          <dd>{state.version}</dd>
          <dt>frontend</dt>
          <dd>
            {state.bundle_embedded
              ? `${state.bundle_files} files compiled in`
              : "not compiled in"}
          </dd>
        </dl>
      )}
    </main>
  );
}
