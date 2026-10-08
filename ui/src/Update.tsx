import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
const Details = lazy(() => import("./UpdateDetails"));
import { updateApply, updateCheck, updateInspect, type Available } from "./api";

export const UPDATE_POLL_MS = 15 * 60 * 1000;

export function useUpdates() {
  const [available, setAvailable] = useState<Available | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [actionProblem, setActionProblem] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [working, setWorking] = useState(false);
  const [installed, setInstalled] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const busy = useRef(false);
  const reading = useRef(false);
  const generation = useRef(0);
  const alive = useRef(true);
  const lastCheck = useRef(0);

  const check = useCallback(async (force = false) => {
    if (busy.current || reading.current) return;
    reading.current = true;
    const request = ++generation.current;
    setChecking(true);
    try {
      const result = await updateCheck(force);
      if (!alive.current || request !== generation.current) return;
      setAvailable(result);
      setProblem(null);
    } catch (error: unknown) {
      if (alive.current && request === generation.current)
        setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      if (alive.current && request === generation.current) {
        reading.current = false;
        lastCheck.current = Date.now();
        setChecking(false);
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void check();
    const poll = () => {
      if (document.visibilityState !== "hidden" && Date.now() - lastCheck.current >= UPDATE_POLL_MS)
        void check();
    };
    const timer = window.setInterval(poll, UPDATE_POLL_MS);
    window.addEventListener("focus", poll);
    document.addEventListener("visibilitychange", poll);
    return () => {
      alive.current = false;
      generation.current++;
      reading.current = false;
      window.clearInterval(timer);
      window.removeEventListener("focus", poll);
      document.removeEventListener("visibilitychange", poll);
    };
  }, [check]);

  const install = async () => {
    if (busy.current || !available?.can_update || !available.latest || !available.approval) return;
    // Pin the exact review, even if a later read resolves after this click.
    const { latest, approval } = available;
    busy.current = true;
    generation.current++;
    reading.current = false;
    setChecking(false);
    setWorking(true);
    setActionProblem(null);
    try {
      const result = await updateApply(latest, approval);
      if (alive.current) {
        setInstalled(result.version);
        setNotice([`Recovery backup: ${result.backup}`, ...(result.warnings ?? [])].join(" · "));
      }
    } catch (error: unknown) {
      if (alive.current) setActionProblem(error instanceof Error ? error.message : String(error));
    } finally {
      busy.current = false;
      if (alive.current) { setWorking(false); void check(true); }
    }
  };
  const inspect = async () => {
    if (busy.current) return;
    busy.current = true;
    generation.current++;
    reading.current = false;
    setChecking(false);
    setWorking(true);
    setActionProblem(null);
    try {
      const result = await updateInspect();
      if (alive.current && result.installed) setInstalled(result.version);
    } catch (error: unknown) {
      if (alive.current) setActionProblem(error instanceof Error ? error.message : String(error));
    } finally {
      busy.current = false;
      if (alive.current) { setWorking(false); void check(true); }
    }
  };
  return { available, problem: actionProblem ?? problem, checking, working: working || available?.state === "updating", installed, notice, check, install, inspect };
}

export type Updates = ReturnType<typeof useUpdates>;

export function UpdateBanner({ updates }: { updates: Updates }) {
  const [review, setReview] = useState(false);
  const { available, installed, working } = updates;
  if (!working && available?.state !== "inspection" && (installed || available?.state === "restart"))
    return <div className="update update--banner" role="status"><span>{installed ? `Installed ${installed}.` : "The installed files changed."} <strong>Restart AI Team</strong> to load the updated files. Save unsaved edits before quitting.</span>{updates.notice && <p>{updates.notice}</p>}</div>;
  if (!available?.can_update && !available?.update_available && available?.state !== "inspection" && !working) return null;
  return <div className="update update--banner">
    <div className="update__heading"><span>{working ? "Updating AI Team…" : available?.state === "inspection" ? "An interrupted update needs inspection." : available?.latest === available?.current ? `An installed program needs updating to ${available?.latest}.` : `${available?.latest} is available — you have ${available?.current}.`}</span>
      <button type="button" className="button" onClick={() => setReview(value => !value)} aria-expanded={review}>{review ? "Hide update details" : "Review update"}</button>
    </div>
    {review && <UpdateDetails updates={updates} />}
    {working && <div className="update__bar" aria-label="updating" />}
  </div>;
}

export function UpdateDetails({ updates }: { updates: Updates }) {
  return <Suspense fallback={<p className="faint">Loading update details…</p>}><Details updates={updates} /></Suspense>;
}
