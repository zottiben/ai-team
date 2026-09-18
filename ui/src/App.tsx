import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Board } from "./Board";
import { Approvals, Prompt, Seats } from "./Console";
import { Today } from "./Today";
import {
  approvals as fetchApprovals,
  health,
  projects as fetchProjects,
  run as fetchRun,
  runEvents as fetchRunEvents,
  runs as fetchRuns,
  subscribe,
  type Approval,
  type Health,
  type Project,
  type Run,
  type RunDetail,
  type RunEvent,
} from "./api";
import { apply, followSystem, stored, type Theme } from "./theme";

/**
 * The shell: sidebar, main, right dock.
 *
 * One surface at a time in the dock, one overlay at a time, and Esc always dismisses the
 * topmost thing. That last rule is why the key handler lives here rather than in each
 * surface - Esc has to know what is on top, and only this level does.
 */
/** Named once, near the type, rather than in a ternary chain that grows a branch per view. */
const VIEW_NAMES = { today: "Today", console: "Console", board: "Board" } as const;

export default function App() {
  const [theme, setTheme] = useState<Theme>(stored);
  const [projects, setProjects] = useState<Project[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [project, setProject] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [pending, setPending] = useState<Approval[]>([]);
  const [view, setView] = useState<"today" | "console" | "board">("today");
  const [overlay, setOverlay] = useState<null | "about">(null);
  // Bumped on every server tick, so the board re-reads without owning a subscription.
  const [tick, setTick] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [info, setInfo] = useState<Health | null>(null);

  useEffect(() => {
    apply(theme);
    return followSystem(() => theme);
  }, [theme]);

  useEffect(() => {
    health().then(setInfo).catch(() => setInfo(null));
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [nextProjects, nextRuns] = await Promise.all([
        fetchProjects(),
        fetchRuns(project ?? undefined),
      ]);
      setProjects(nextProjects);
      setRuns(nextRuns);
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project]);

  // The selected run is refetched separately: it changes far more often than the lists,
  // and a turn in flight should not redraw the sidebar on every event.
  const refreshSelected = useCallback(async () => {
    if (selected === null) {
      setDetail(null);
      setEvents([]);
      setPending([]);
      return;
    }
    try {
      // The approvals are an addition to this panel, not the point of it. Folding them
      // into the same failure as the run itself means one bad response blanks a view
      // that could have rendered everything else.
      const [next, nextEvents] = await Promise.all([
        fetchRun(selected),
        fetchRunEvents(selected),
      ]);
      setDetail(next);
      setEvents(nextEvents);
      setPending(await fetchApprovals(selected).catch(() => []));
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [selected]);

  useEffect(() => {
    void refresh();
  }, [refresh]);
  useEffect(() => {
    void refreshSelected();
  }, [refreshSelected]);

  // One subscription for the window. The tick says only that the database moved; what
  // that means depends on what is on screen, so both views re-read.
  const onTick = useRef<() => void>(() => {});
  onTick.current = () => {
    void refresh();
    void refreshSelected();
    setTick((value) => value + 1);
  };
  useEffect(() => subscribe(() => onTick.current()), []);

  // Esc dismisses the topmost surface, innermost first. A single handler, because
  // "topmost" is a fact about the whole shell.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (overlay !== null) setOverlay(null);
      else if (expanded) setExpanded(false);
      else if (selected !== null) setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [overlay, expanded, selected]);

  const shown = useMemo(
    () => (project === null ? runs : runs.filter((r) => r.project_id === project)),
    [runs, project],
  );

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="sidebar__brand">
          <h1>ai-team</h1>
          <span className="sidebar__version">{info?.version ?? ""}</span>
        </div>

        <nav className="sidebar__section" aria-label="Views">
          <span className="sidebar__label">View</span>
          {(["today", "console", "board"] as const).map((option) => (
            <button
              type="button"
              key={option}
              className="nav-item"
              aria-current={view === option}
              onClick={() => setView(option)}
            >
              <span>{VIEW_NAMES[option]}</span>
            </button>
          ))}
        </nav>

        <nav className="sidebar__section" aria-label="Projects">
          <span className="sidebar__label">Projects</span>
          <button
            type="button"
            className="nav-item"
            aria-current={project === null}
            onClick={() => setProject(null)}
          >
            <span>Everything</span>
            <span className="nav-item__count">{runs.length}</span>
          </button>
          {projects.map((entry) => (
            <button
              type="button"
              key={entry.id}
              className="nav-item"
              aria-current={project === entry.id}
              onClick={() => setProject(entry.id)}
            >
              <span>{entry.name}</span>
              <span className="nav-item__count">{entry.open_runs > 0 ? entry.open_runs : ""}</span>
            </button>
          ))}
          {projects.length === 0 && <span className="faint">None yet - `ait init`.</span>}
        </nav>

        <div className="sidebar__section" style={{ marginTop: "auto" }}>
          <span className="sidebar__label">Theme</span>
          {(["system", "dark", "light"] as const).map((option) => (
            <button
              type="button"
              key={option}
              className="nav-item"
              aria-current={theme === option}
              onClick={() => setTheme(option)}
            >
              <span>{option}</span>
            </button>
          ))}
          <button type="button" className="nav-item" onClick={() => setOverlay("about")}>
            <span>About</span>
            <span className="kbd">Esc</span>
          </button>
        </div>
      </aside>

      <main className="main">
        {view === "today" && (
          <Today
            tick={tick}
            onOpenRun={(id) => {
              // Today answers "what now"; the run itself is the Console's job, so
              // following an item takes you there rather than growing a third surface
              // that renders runs slightly differently.
              setView("console");
              setSelected(id);
            }}
          />
        )}

        {view === "board" && (
          <Board
            project={projects.find((entry) => entry.id === project)?.slug ?? null}
            tick={tick}
          />
        )}

        {view === "console" && (
          <>
        <div className="main__header">
          <h2>Console</h2>
          {problem !== null && <span className="error">{problem}</span>}
        </div>

        <Prompt
          project={projects.find((entry) => entry.id === project)?.slug ?? null}
          onStarted={() => void refresh()}
        />

        <div className="main__header">
          <h2>Runs</h2>
        </div>

        {shown.length === 0 && (
          <p className="empty">
            No runs yet. Start one with <code className="mono">ait run -p &lt;project&gt;</code> and
            it will appear here.
          </p>
        )}

        <div className="list">
          {shown.map((entry) => (
            <button
              type="button"
              key={entry.id}
              className="card"
              onClick={() => setSelected(entry.id)}
            >
              <div className="card__row">
                <span className="status" data-status={entry.status}>
                  {entry.status}
                </span>
                <span className="faint mono">#{entry.id}</span>
              </div>
              <span>{entry.prompt}</span>
            </button>
          ))}
        </div>
          </>
        )}
      </main>

      {/* One surface at a time, and it collapses rather than covering what it describes. */}
      {detail !== null && (
        <aside className="dock">
          <div className="dock__header">
            <span className="dock__title">Run #{detail.id}</span>
            <button type="button" className="button" onClick={() => setSelected(null)}>
              Close
            </button>
          </div>

          <span className="status" data-status={detail.status}>
            {detail.status}
          </span>
          <p className="muted">{detail.prompt}</p>

          <Approvals run={detail} pending={pending} onAnswered={() => void refreshSelected()} />

          <span className="dock__title">Team</span>
          <Seats nodes={detail.nodes} />

          <div className="card__row">
            <span className="dock__title">Spent</span>
            <span className="mono faint">
              {detail.usage.tokens_in + detail.usage.tokens_out + detail.usage.cache_write} billable
            </span>
          </div>

          <div className="card__row">
            <span className="dock__title">Events</span>
            <button type="button" className="button" onClick={() => setExpanded(true)}>
              Expand
            </button>
          </div>
          <Events events={events.slice(-25)} />
        </aside>
      )}

      {/* A full-window view, for when the dock is too narrow to read a turn in. */}
      {expanded && detail !== null && (
        <section className="full-view">
          <div className="main__header">
            <h2>Run #{detail.id}</h2>
            <button type="button" className="button" onClick={() => setExpanded(false)}>
              Close <span className="kbd">Esc</span>
            </button>
          </div>
          <Events events={events} />
        </section>
      )}

      {overlay === "about" && (
        <div
          className="scrim"
          role="presentation"
          onClick={(event) => event.target === event.currentTarget && setOverlay(null)}
        >
          <div className="overlay" role="dialog" aria-modal="true" aria-label="About ai-team">
            <h2 style={{ margin: 0 }}>ai-team</h2>
            <p className="muted">
              A team of agents, working your plan in leased worktrees. This window is a view
              over the same database the CLI writes - nothing here holds state of its own.
            </p>
            <dl className="mono">
              <dt className="faint">version</dt>
              <dd>{info?.version ?? "unknown"}</dd>
              <dt className="faint">frontend</dt>
              <dd>
                {info?.bundle_embedded === true
                  ? `${info.bundle_files} files compiled in`
                  : "not compiled in"}
              </dd>
            </dl>
            <button type="button" className="button button--primary" onClick={() => setOverlay(null)}>
              Close
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function Events({ events }: { events: RunEvent[] }) {
  if (events.length === 0) {
    return <p className="faint">Nothing recorded yet.</p>;
  }
  return (
    <div className="events">
      {events.map((event) => (
        <div key={event.id} className="event">
          <span className="event__kind">{event.kind}</span>
          <span>{event.summary}</span>
        </div>
      ))}
    </div>
  );
}
