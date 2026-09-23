import { useCallback, useEffect, useState } from "react";

import { Browse } from "./Browse";
import { SignIn } from "./SignIn";
import {
  doctor,
  doctorFix,
  registerProject,
  setProvider,
  settings as fetchSettings,
  type Check,
  type DoctorReport,
  type ProviderSetting,
} from "./api";

/**
 * Taking a fresh install to a working one.
 *
 * Deliberately not a wizard with its own state. Every step is a *view of the readiness
 * report*: it appears when the report says something is wrong and disappears when the
 * report says it is fixed. A three-step sequence with a remembered position gets out of
 * step with reality the first time somebody fixes something in a terminal, and then it is
 * lying about a machine it can see.
 *
 * Which also means there is no "finish". The page is done when there is nothing left on
 * it, and closing it early is fine because it is not holding anything.
 */
export function Setup({ onReady }: { onReady: () => void }) {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [providers, setProviders] = useState<ProviderSetting[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [browsing, setBrowsing] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const found = await doctor();
      setReport(found);
      // Providers need the settings route; on a machine with no profile it still answers,
      // with everything denied.
      setProviders((await fetchSettings().catch(() => null))?.providers ?? []);
      setProblem(null);
      if (!found.needs_setup) onReady();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (what: () => Promise<unknown>, label: string) => {
    setBusy(label);
    try {
      await what();
      await load();
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  };

  if (report === null) {
    return <p className="empty">{problem ?? "Having a look at this machine…"}</p>;
  }

  const check = (id: string) => report.checks.find((entry) => entry.id === id);
  const broken = (id: string) => {
    const found = check(id);
    return found !== undefined && found.severity !== "fine";
  };

  // What ai-team can repair on its own (D17). Offered one at a time and named, because
  // "fix everything" is a button people press without reading.
  const ours = report.checks.filter(
    (entry) => entry.severity !== "fine" && entry.fix.by === "itself",
  );
  const commands = report.checks.filter(
    (entry) => entry.severity !== "fine" && entry.fix.by === "command",
  );
  const usable = providers.filter((provider) => provider.reachable);

  return (
    <div className="setup">
      <div className="main__header">
        <h2>Let's get you set up</h2>
        <span className="faint">ai-team {report.version}</span>
      </div>
      {problem !== null && <p className="error">{problem}</p>}

      {ours.length > 0 && (
        <section className="settings__group">
          <h3>Ready when you are</h3>
          <p className="faint">These are ai-team's own files. It can create them now.</p>
          {ours.map((entry) => (
            <div key={entry.id} className="settings__row">
              <span className="settings__toggle">{entry.label}</span>
              <span className="faint settings__detail">
                {"describe" in entry.fix ? entry.fix.describe : entry.detail}
              </span>
              <button
                type="button"
                className="button button--primary"
                disabled={busy !== null}
                onClick={() => {
                  if (entry.fix.by !== "itself") return;
                  const action = entry.fix.action;
                  void act(() => doctorFix(action), entry.id);
                }}
              >
                {busy === entry.id ? "Working…" : "Do it"}
              </button>
            </div>
          ))}
        </section>
      )}

      {broken("providers") && (
        <section className="settings__group">
          <h3>Pick an account to think with</h3>
          <p className="faint">
            Only subscriptions and local models - never a metered API key. Tick one, and
            sign in here if it is not already.
          </p>
          {providers.map((provider) => (
            <div key={provider.provider} className="settings__source">
              <div className="settings__row">
                <label className="settings__toggle">
                  <input
                    type="checkbox"
                    aria-label={`allow ${provider.provider}`}
                    checked={provider.allowed}
                    disabled={busy !== null}
                    onChange={(event) =>
                      void act(
                        () => setProvider(provider.provider, event.target.checked),
                        provider.provider,
                      )
                    }
                  />
                  <span>{provider.label}</span>
                </label>
                {/* Ticked and not signed in is the commonest mistake, and it fails minutes
                    later inside a run - so it is said here. */}
                <span
                  className="status"
                  data-status={
                    !provider.allowed ? "queued" : provider.reachable ? "done" : "failed"
                  }
                >
                  {!provider.allowed ? "off" : provider.reachable ? "ready" : "not signed in"}
                </span>
                <span className="faint settings__detail">{provider.how}</span>
              </div>
              {/* Only where it is the thing standing in the way. A sign-in button beside a
                  provider that is already answering is a button whose only use is to log
                  somebody out of an account that was working. */}
              {provider.allowed && !provider.reachable && (
                <SignIn provider={provider} onDone={() => void load()} />
              )}
            </div>
          ))}
        </section>
      )}

      {!broken("providers") && broken("projects") && (
        <section className="settings__group">
          <h3>What are we working on?</h3>
          <p className="faint">
            The path to a repository, or find it by browsing. ai-team reads its shape and
            gives each seat the part it owns.
          </p>
          <form
            className="projects__add"
            onSubmit={(event) => {
              event.preventDefault();
              if (path.trim() === "") return;
              void act(() => registerProject({ path: path.trim() }), "project");
            }}
          >
            <input
              aria-label="path"
              placeholder="~/src/your-repo"
              value={path}
              onChange={(event) => setPath(event.target.value)}
            />
            <button
              type="button"
              className="button"
              aria-expanded={browsing}
              onClick={() => setBrowsing(!browsing)}
            >
              {browsing ? "Close" : "Browse…"}
            </button>
            <button type="submit" className="button button--primary" disabled={busy !== null}>
              {busy === "project" ? "Adding…" : "Add"}
            </button>
          </form>
          {/* Picking fills the field rather than registering directly, so what gets
              submitted is always the path that is written down and checkable. */}
          {browsing && (
            <Browse
              onPick={(picked) => {
                setPath(picked);
                setBrowsing(false);
              }}
            />
          )}
        </section>
      )}

      {commands.length > 0 && (
        <section className="settings__group">
          <h3>Tools ai-team borrows</h3>
          {/* D17: never a button that runs this. `curl | sh` from a GUI is a decision that
              is not ai-team's to take, so the command is shown and copied. */}
          <p className="faint">
            These are separate tools. Run each in a terminal - ai-team will not install
            software on your machine.
          </p>
          {commands.map((entry) => (
            <div key={entry.id} className="setup__command">
              <div className="card__row">
                <span>{entry.label}</span>
                <span
                  className="status"
                  data-status={entry.severity === "blocking" ? "failed" : "parked"}
                >
                  {entry.severity === "blocking" ? "needed" : "optional"}
                </span>
              </div>
              <span className="faint">{"why" in entry.fix ? entry.fix.why : entry.detail}</span>
              <div className="card__row">
                <code className="mono setup__run">{"run" in entry.fix ? entry.fix.run : ""}</code>
                <button
                  type="button"
                  className="button"
                  onClick={() => {
                    const run = "run" in entry.fix ? entry.fix.run : "";
                    void navigator.clipboard?.writeText(run);
                    setCopied(entry.id);
                  }}
                >
                  {copied === entry.id ? "Copied" : "Copy"}
                </button>
              </div>
            </div>
          ))}
        </section>
      )}

      {/* Only once there is genuinely nothing blocking, so it cannot congratulate somebody
          on a machine that still cannot run anything. */}
      {report.can_run && usable.length > 0 && (
        <section className="settings__group">
          <h3>That's it</h3>
          <p className="faint">
            {usable.length === 1
              ? `${usable[0]?.label} is ready.`
              : `${usable.map((provider) => provider.label).join(" and ")} are ready.`}{" "}
            Give the team something to build.
          </p>
          <button type="button" className="button button--primary" onClick={onReady}>
            Open ai-team
          </button>
        </section>
      )}
    </div>
  );
}

/**
 * What is wrong, from wherever you are.
 *
 * Reads the same report and shows only what is blocking, because a banner that appears for
 * a missing file-sql is a banner people learn to ignore - and then it is not there when the
 * database has gone.
 */
export function HealthBanner({ tick, onOpen }: { tick: number; onOpen: () => void }) {
  const [report, setReport] = useState<DoctorReport | null>(null);

  useEffect(() => {
    // Silence on failure: a window that cannot reach its own API has a bigger problem than
    // this banner, and every other surface is already saying so.
    void doctor()
      .then(setReport)
      .catch(() => {});
  }, [tick]);

  if (report === null || report.severity !== "blocking") return null;

  const worst: Check | undefined = report.checks.find((check) => check.severity === "blocking");

  return (
    <button type="button" className="health" onClick={onOpen}>
      <span className="status" data-status="failed">
        needs attention
      </span>
      <span>{worst?.label ?? "Something is wrong"}</span>
      <span className="faint">{worst?.detail ?? ""}</span>
    </button>
  );
}
