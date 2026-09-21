import { useCallback, useEffect, useRef, useState } from "react";

import { activityOf, isBusy, type Activity } from "./activity";
import { doingLabel } from "./Crew";
import { RepoGraph, zoneColour } from "./Map";
import {
  analytics as fetchAnalytics,
  board as fetchBoard,
  crew as fetchCrew,
  repoMap as fetchMap,
  roster as fetchRoster,
  runs as fetchRuns,
  BOARD_COLUMNS,
  type AnalyticsRow,
  type Board,
  type Member,
  type Project,
  type RepoMap,
  type Roster,
  type Run,
} from "./api";
import type { WorkspaceView } from "./Workspace";

/** What the page re-reads on every tick. The map is not in here on purpose. */
type Live = {
  board: Board;
  roster: Roster;
  crew: Member[];
  runs: Run[];
  gates: AnalyticsRow[];
};

/** Thousands, because the interesting numbers are six figures. */
function compact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

function percent(value: number | null): string {
  return value === null ? "--" : `${Math.round(value * 100)}%`;
}

/**
 * A number that shows when it moves.
 *
 * Token and turn counters climb while a turn runs, and a figure that silently replaces
 * itself between two renders reads as static. This flashes on an increase, which is the
 * difference between a dashboard that reports activity and one that looks like it is
 * having it.
 */
function Ticking({ value, format }: { value: number; format?: (n: number) => string }) {
  const seen = useRef(value);
  const [bumped, setBumped] = useState(false);

  useEffect(() => {
    if (value > seen.current) {
      setBumped(true);
      const timer = setTimeout(() => setBumped(false), 600);
      seen.current = value;
      return () => clearTimeout(timer);
    }
    seen.current = value;
    return undefined;
  }, [value]);

  return (
    <span className="ticking" data-bumped={bumped}>
      {format === undefined ? value : format(value)}
    </span>
  );
}

/** `2m 14s`, counted from a timestamp the server wrote. */
function since(started: string | null, now: number): string | null {
  if (started === null) return null;
  const at = Date.parse(started);
  if (Number.isNaN(at)) return null;
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/**
 * A clock that only runs while something is.
 *
 * The database tick says a row changed, not that a second passed, so an elapsed time
 * driven by it freezes between events - which looks broken precisely during a long
 * thinking turn. This ticks on its own, and stops entirely when nothing is live so an
 * idle window is not re-rendering once a second for ever.
 */
function useNow(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return undefined;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}

/**
 * One repository, at a glance, and in motion while it is being worked on.
 *
 * The landing view. What this checkout is, who owns which part of it, and - when a run is
 * up - which paths are being edited right now, by whom, and how far along it is. Every
 * number is read from the same database the CLI writes; the empty states say so plainly
 * rather than drawing an impressive shape out of nothing.
 *
 * The split between what is refetched and what is not matters. The map is a filesystem
 * walk and the shape of a repository does not change because a token arrived, so it is
 * read once per project. Everything else is SQL or the planner, and is re-read on every
 * tick - which is what makes the picture move.
 */
export function Overview({
  project,
  tick,
  onGo,
}: {
  project: Project;
  tick: number;
  onGo: (view: WorkspaceView) => void;
}) {
  const [map, setMap] = useState<RepoMap | null>(null);
  const [live, setLive] = useState<Live | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Once per project: a walk of the checkout on every tick would be several hundred
  // `read_dir` calls a second during a run, to redraw a shape that has not changed.
  useEffect(() => {
    let current = true;
    fetchMap(project.slug)
      .then((found) => {
        if (current) setMap(found);
      })
      .catch((error: unknown) => {
        if (current) setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [project.slug]);

  const load = useCallback(async () => {
    try {
      const [board, roster, crew, runs, gates] = await Promise.all([
        fetchBoard(project.slug),
        fetchRoster(project.slug),
        fetchCrew(project.slug),
        // Runs are scoped by id and everything else by slug, because that is what each
        // route takes - resolving one into the other here would invent a mapping the
        // server already owns.
        fetchRuns(project.id),
        fetchAnalytics("agent", project.slug),
      ]);
      setLive({ board, roster, crew, runs, gates });
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project.slug, project.id]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  const running = live?.runs.find((run) => run.status === "running") ?? null;
  const now = useNow(running !== null);

  if (problem !== null) return <p className="error">{problem}</p>;
  if (live === null) return <p className="empty">Reading the checkout…</p>;

  const { board, roster, crew, runs, gates } = live;
  const activity: Activity =
    map === null
      ? { hot: new Map(), warm: new Map(), live: new Map(), working: new Set() }
      : activityOf(map, crew, board.slices);
  const busy = isBusy(activity);

  const roles = [...new Set((map?.zones ?? []).map((zone) => zone.role))].sort();
  const working = crew.filter((member) => activity.working.has(member.role));
  const waiting = crew.filter((member) => member.doing === "parked");

  const owned = map === null ? 0 : map.files - map.unowned;
  const counted = (status: string) =>
    board.slices.filter((slice) => slice.status === status).length;

  const gatesRun = gates.reduce((sum, row) => sum + row.gates_run, 0);
  const tokens = crew.reduce((sum, member) => sum + member.tokens_in + member.tokens_out, 0);
  const turns = crew.reduce((sum, member) => sum + member.turns, 0);
  const elapsed = since(running?.started_at ?? null, now);

  return (
    <div className="overview" data-busy={busy}>
      <div className="main__header">
        <h2>{project.name}</h2>
        {running === null ? (
          <span className="status" data-status="queued">
            nothing running
          </span>
        ) : (
          <span className="status" data-status="running">
            run #{running.id}
            {elapsed === null ? "" : ` · ${elapsed}`}
          </span>
        )}
      </div>

      {/* --- the numbers ---------------------------------------------------------- */}
      <div className="figures">
        <div className="figure">
          <span className="figure__label faint">files</span>
          <span className="figure__value mono">{map === null ? "…" : compact(map.files)}</span>
        </div>
        <div className="figure">
          <span className="figure__label faint">owned</span>
          <span className="figure__value mono">
            {map === null || map.files === 0 ? "--" : percent(owned / map.files)}
          </span>
        </div>
        <div className="figure">
          <span className="figure__label faint">slices</span>
          <span className="figure__value mono">{board.slices.length}</span>
        </div>
        <div className="figure">
          <span className="figure__label faint">turns</span>
          <span className="figure__value mono">
            <Ticking value={turns} />
          </span>
        </div>
        <div className="figure">
          <span className="figure__label faint">tokens</span>
          <span className="figure__value mono">
            <Ticking value={tokens} format={compact} />
          </span>
        </div>
      </div>

      {/* --- the map -------------------------------------------------------------- */}
      <section className="block">
        <div className="block__head">
          <h3>Map</h3>
          <span className="faint">
            {busy
              ? `${activity.hot.size > 0 ? activity.hot.size : activity.warm.size} paths lit · ${working.length} working`
              : "every path, and the seat whose zone claims it"}
          </span>
        </div>

        {map === null ? (
          <p className="empty">Walking the checkout…</p>
        ) : (
          <>
            <ul className="legend">
              {map.zones
                .filter((zone) => zone.owns > 0)
                .sort((a, b) => b.owns - a.owns)
                .map((zone) => (
                  <li key={zone.role} data-working={activity.working.has(zone.role)}>
                    <span
                      className="legend__swatch"
                      style={{ background: zoneColour(zone.role, roles) }}
                    />
                    {zone.role}
                    <span className="faint">{zone.owns}</span>
                  </li>
                ))}
              {map.unowned > 0 && (
                <li>
                  <span
                    className="legend__swatch"
                    style={{ background: "var(--zone-none-default)" }}
                  />
                  unowned
                  <span className="faint">{map.unowned}</span>
                </li>
              )}
            </ul>

            <RepoGraph map={map} activity={activity} />

            <p className="faint block__foot">
              {map.unowned === 0
                ? "Every path has an owner - nothing here would be reported undone."
                : `${map.unowned} ${map.unowned === 1 ? "path belongs" : "paths belong"} to no zone. A slice touching one is reported undone rather than given to somebody.`}
            </p>
          </>
        )}
      </section>

      {/* --- who is doing what right now ------------------------------------------ */}
      <section className="block">
        <div className="block__head">
          <h3>Crew</h3>
          <span className="faint">
            {working.length === 0 ? "nobody is working" : `${working.length} working`}
            {waiting.length > 0 ? ` · ${waiting.length} waiting on you` : ""}
          </span>
          <button type="button" className="block__go" onClick={() => onGo("work")}>
            work →
          </button>
        </div>

        {crew.length === 0 ? (
          <p className="empty">This project has no team yet.</p>
        ) : (
          <ul className="seats">
            {crew.map((member) => {
              const up = activity.working.has(member.role);
              return (
                <li
                  key={member.agent_id}
                  className="seats__seat"
                  data-doing={member.doing}
                  style={
                    {
                      "--seat-mark": up
                        ? zoneColour(member.role, roles)
                        : "var(--border-subtle-default)",
                    } as React.CSSProperties
                  }
                >
                  <span className="status" data-status={up ? "running" : member.doing}>
                    {member.name}
                  </span>
                  <span className="faint seats__what">
                    {member.slice_key ?? doingLabel(member.doing)}
                  </span>
                  {up && (
                    <span className="mono seats__meter">
                      <Ticking value={member.turns} /> turns ·{" "}
                      <Ticking value={member.tokens_in + member.tokens_out} format={compact} />
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {/* --- the board ------------------------------------------------------------ */}
      <section className="block">
        <div className="block__head">
          <h3>Board</h3>
          <span className="faint">{board.plan?.title ?? "no plan yet"}</span>
          <button type="button" className="block__go" onClick={() => onGo("board")}>
            board →
          </button>
        </div>

        {board.plan === null ? (
          <p className="empty">
            No plan on this checkout. <code>aip new</code>, or start a run and the
            orchestrator writes one.
          </p>
        ) : (
          <>
            <ol className="path">
              {BOARD_COLUMNS.map((column) => (
                <li
                  key={column}
                  className="path__stage"
                  data-empty={counted(column) === 0}
                  data-active={column === "active" && counted(column) > 0}
                >
                  <span className="faint">{column.replace("_", " ")}</span>
                  <span className="mono path__count">{counted(column)}</span>
                </li>
              ))}
            </ol>
            {/* One mark per slice, in plan order - the shape of the work, not a chart. */}
            <div className="tape">
              {board.slices.map((slice) => (
                <span
                  key={slice.key}
                  className="tape__tick"
                  data-status={slice.status}
                  data-live={working.some((member) => member.slice_key === slice.key)}
                  title={`${slice.key} · ${slice.status}`}
                />
              ))}
            </div>
          </>
        )}
      </section>

      {/* --- seats and gates ------------------------------------------------------ */}
      <div className="columns">
        <section className="block">
          <div className="block__head">
            <h3>Seats</h3>
            <span className="faint">zone, model, and what it owns here</span>
            <button type="button" className="block__go" onClick={() => onGo("team")}>
              team →
            </button>
          </div>
          <ul className="rack">
            {roster.seats.map((seat) => {
              const owns = map?.zones.find((zone) => zone.role === seat.role)?.owns ?? 0;
              return (
                <li key={seat.id} className="rack__seat" data-off={!seat.enabled}>
                  <span
                    className="rack__mark"
                    style={{
                      // A seat that owns nothing gets the neutral mark, so a colour on
                      // this page always means "find me on the map".
                      background:
                        owns === 0 ? "var(--zone-none-default)" : zoneColour(seat.role, roles),
                    }}
                  />
                  <span className="rack__name">{seat.name}</span>
                  <span className="faint mono rack__model">
                    {seat.effective_provider}/{seat.effective_model}
                  </span>
                  {/* A zone that claims nothing is the interesting case: that seat will
                      never be given work, and the roster's text alone cannot say so. */}
                  <span className="mono rack__owns" data-none={owns === 0}>
                    {owns === 0 ? "owns nothing" : `${owns}`}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>

        <section className="block">
          <div className="block__head">
            <h3>Gates</h3>
            <span className="faint">this repo's own checks, discovered not assumed</span>
          </div>
          {gatesRun === 0 ? (
            <p className="empty">
              No gate has run here yet. They are discovered from this repo's manifests when
              the first node finishes.
            </p>
          ) : (
            <ul className="bars">
              {gates
                .filter((row) => row.gates_run > 0)
                .map((row) => (
                  <li key={row.group}>
                    <span className="bars__name">{row.group}</span>
                    <span className="bars__track">
                      <span
                        className="bars__fill"
                        style={{
                          width: `${Math.round((row.gates_passed / row.gates_run) * 100)}%`,
                        }}
                      />
                    </span>
                    <span className="mono faint">{percent(row.gate_pass_rate)}</span>
                  </li>
                ))}
            </ul>
          )}
        </section>
      </div>

      {/* --- recent runs ---------------------------------------------------------- */}
      <section className="block">
        <div className="block__head">
          <h3>Runs</h3>
          <span className="faint">{runs.length === 0 ? "none yet" : `${runs.length} here`}</span>
        </div>
        {runs.length === 0 ? (
          <p className="empty">
            Nothing has run here yet. Say what you want built and the team will plan it.
          </p>
        ) : (
          <div className="list">
            {runs.slice(0, 5).map((run) => (
              <div key={run.id} className="card" data-live={run.status === "running"}>
                <div className="card__row">
                  <span className="status" data-status={run.status}>
                    {run.status}
                  </span>
                  <span className="faint mono">
                    #{run.id}
                    {run.status === "running" && elapsed !== null ? ` · ${elapsed}` : ""}
                  </span>
                </div>
                <span>{run.prompt}</span>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
