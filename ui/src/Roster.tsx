import { useCallback, useEffect, useState } from "react";

import {
  editSeat,
  projects as fetchProjects,
  roster as fetchRoster,
  type Project,
  type Roster as RosterData,
} from "./api";

/**
 * Who is working on what.
 *
 * The one thing this page refuses to do is show a seat as configured when dispatch will
 * resolve it differently. A seat set to a provider this machine denies falls back through
 * the ranking (D13), and a roster that hides that lies about which account the work lands
 * on - so both are shown, and the difference is called out rather than smoothed over.
 *
 * With no project selected it shows what a *new* project would get, which is the question
 * somebody asks before creating one rather than after.
 */
export function Roster({ onChanged }: { onChanged: () => void }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const [data, setData] = useState<RosterData | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setProjects(await fetchProjects().catch(() => []));
      setData(await fetchRoster(chosen));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [chosen]);

  useEffect(() => {
    void load();
  }, [load]);

  const change = async (id: number, what: Parameters<typeof editSeat>[1]) => {
    try {
      await editSeat(id, what);
      await load();
      onChanged();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="roster">
      <div className="main__header">
        <h2>Team</h2>
        <label className="settings__toggle">
          <span className="faint">for</span>
          <select
            aria-label="project"
            value={chosen ?? ""}
            onChange={(event) => setChosen(event.target.value === "" ? null : event.target.value)}
          >
            <option value="">a new project (defaults)</option>
            {projects.map((project) => (
              <option key={project.id} value={project.slug}>
                {project.name}
              </option>
            ))}
          </select>
        </label>
        {data?.team !== null && data?.team !== undefined && (
          <span className="faint mono">{data.team}</span>
        )}
      </div>

      {problem !== null && <p className="error">{problem}</p>}
      {data === null && <p className="empty">Reading the roster…</p>}

      {data !== null && data.project === null && (
        <p className="faint">
          These are the seats a new project gets, on the best provider this machine can
          reach. Change them per project once it exists.
        </p>
      )}

      {data !== null && data.available.length === 0 && (
        <p className="error">
          No provider is available, so none of these seats can think. Allow one in Settings.
        </p>
      )}

      <div className="list">
        {data?.seats.map((seat) => {
          const differs =
            seat.effective_provider !== seat.provider || seat.effective_model !== seat.model;
          return (
            <div key={`${seat.role}-${seat.id}`} className="card">
              <div className="card__row">
                <span>{seat.name}</span>
                <span className="faint mono">{seat.role}</span>
                <span className="status" data-status={seat.read_only ? "queued" : "running"}>
                  {seat.read_only ? "reads" : "writes"}
                </span>
                {!seat.enabled && (
                  <span className="status" data-status="cancelled">
                    off
                  </span>
                )}
              </div>
              <span className="faint">{seat.purpose}</span>

              <div className="card__row">
                <label className="settings__toggle">
                  <span className="faint">model</span>
                  <select
                    aria-label={`provider for ${seat.role}`}
                    value={seat.provider}
                    disabled={seat.id < 0}
                    onChange={(event) => void change(seat.id, { provider: event.target.value })}
                  >
                    {/* The configured provider is listed even when unavailable, or the
                        picker would silently show something the seat is not set to. */}
                    {[...new Set([seat.provider, ...data.available])].map((name) => (
                      <option key={name} value={name}>
                        {name}
                        {data.available.includes(name) ? "" : " (unavailable)"}
                      </option>
                    ))}
                  </select>
                </label>
                <span className="faint mono">{seat.model}</span>
              </div>

              {/* The whole point: what it will actually be, when that is not what it says. */}
              {differs && (
                <span className="notice">
                  Runs as {seat.effective_provider}/{seat.effective_model}
                  {seat.fallback_reason === null ? "" : ` - ${seat.fallback_reason}`}
                </span>
              )}

              <div className="card__row">
                <span className="faint">owns</span>
                <span className="mono">
                  {seat.zone.trim() === ""
                    ? seat.read_only
                      ? "nothing - it does not edit"
                      : "nothing yet"
                    : seat.zone.split("\n").join(", ")}
                </span>
              </div>

              {seat.id > 0 && (
                <div className="card__row">
                  <button
                    type="button"
                    className="button"
                    onClick={() => void change(seat.id, { enabled: !seat.enabled })}
                  >
                    {seat.enabled ? "Disable" : "Enable"}
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Said once, because an edit that looks applied and is not is the confusing case. */}
      <p className="faint">
        Changes take effect on the next run - it regenerates the agents before it starts, so
        an edit never half-applies to a run already going.
      </p>
    </div>
  );
}
