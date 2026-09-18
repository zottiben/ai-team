import { useEffect, useState } from "react";

import { updateApply, updateCheck, type Available } from "./api";

/**
 * The update offer.
 *
 * Shown only when there is genuinely something to install and ai-team knows how to
 * install it. A banner saying "up to date" is a banner nobody needs, and one offering an
 * update it cannot perform is worse - it invites a click that ends in an error.
 *
 * Checked once on mount rather than on a timer: an update that appears while somebody is
 * mid-run is not urgent, and a background poll to GitHub from a local tool is a network
 * request nobody asked for.
 */
export function UpdateBanner() {
  const [available, setAvailable] = useState<Available | null>(null);
  const [state, setState] = useState<"idle" | "working" | "done">("idle");
  const [problem, setProblem] = useState<string | null>(null);
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    // Failure is silence: being unable to reach GitHub is not worth a red message in a
    // tool that works perfectly well offline.
    void updateCheck()
      .then(setAvailable)
      .catch(() => {});
  }, []);

  if (state === "done") {
    return (
      <div className="update">
        {/* The restart is the point. A replaced binary does not change the process that
            is already running, so a window that looks updated and is not is the one
            outcome worth preventing. */}
        <span>
          Updated to {version}. <strong>Restart ai-team</strong> to run it.
        </span>
      </div>
    );
  }

  if (available === null || !available.can_update) return null;

  return (
    <div className="update">
      <span>
        {available.latest} is available - you have {available.current}.
      </span>
      {problem !== null && <span className="error">{problem}</span>}
      <button
        type="button"
        className="button button--primary"
        disabled={state === "working"}
        onClick={() => {
          setState("working");
          setProblem(null);
          void updateApply()
            .then((result) => {
              setVersion(result.version);
              setState("done");
            })
            .catch((error: unknown) => {
              setProblem(error instanceof Error ? error.message : String(error));
              setState("idle");
            });
        }}
      >
        {state === "working" ? "Updating…" : "Update"}
      </button>
      {/* Indeterminate on purpose: nobody knows how long a download takes, and a bar
          that invents a percentage is a bar that lies. */}
      {state === "working" && <div className="update__bar" aria-label="updating" />}
    </div>
  );
}
