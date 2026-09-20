import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";

import { Board } from "./Board";
import { Approvals, Prompt, Seats } from "./Console";
import { Review } from "./Review";
import { Roster } from "./Roster";
import { Source } from "./Source";
import {
  approvals as fetchApprovals,
  run as fetchRun,
  runEvents as fetchRunEvents,
  runs as fetchRuns,
  type Approval,
  type Project,
  type Run,
  type RunDetail,
  type RunEvent,
} from "./api";

// Each is most of a megabyte that no other view needs (M4-S18, M4-S20).
const Editor = lazy(async () => ({ default: (await import("./Editor")).Editor }));
const TerminalPane = lazy(async () => ({
  default: (await import("./Terminal")).TerminalPane,
}));

/** The views that belong to one project (D18). */
const VIEWS = {
  work: "Work",
  board: "Board",
  review: "Review",
  editor: "Editor",
  terminal: "Terminal",
  source: "Source",
  team: "Team",
} as const;

export type WorkspaceView = keyof typeof VIEWS;

export const WORKSPACE_VIEWS = Object.keys(VIEWS) as WorkspaceView[];

export function workspaceViewName(view: WorkspaceView): string {
  return VIEWS[view];
}

/**
 * One project, from the inside.
 *
 * This is where the time goes (D18): the crew, the work, the diff, the code. Everything
 * here is about one repository and takes it as a fact rather than as a filter, which is
 * what makes these views answerable at all - "the board" across four projects is not a
 * question with an answer.
 *
 * The run list and its dock live here rather than in the shell, because a run belongs to a
 * project. A shell that owned them would have to decide what they mean when no project is
 * selected, and the honest answer is nothing.
 */
export function Workspace({
  project,
  view,
  tick,
  openRun,
  onOpenedRun,
  onChanged,
}: {
  project: Project;
  view: WorkspaceView;
  tick: number;
  /** A run to open on arrival - how Today hands one over. */
  openRun: number | null;
  onOpenedRun: () => void;
  onChanged: () => void;
}) {
  const [runs, setRuns] = useState<Run[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [pending, setPending] = useState<Approval[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setRuns(await fetchRuns(project.id));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project.id]);

  // The selected run is refetched separately: it changes far more often than the list, and
  // re-reading everything on every tick would make the dock flicker.
  const refreshSelected = useCallback(async () => {
    if (selected === null) {
      setDetail(null);
      setEvents([]);
      setPending([]);
      return;
    }
    try {
      const [found, log] = await Promise.all([fetchRun(selected), fetchRunEvents(selected)]);
      setDetail(found);
      setEvents(log);
      // Supplementary: a bad approvals response must not blank a dock that could render
      // everything else (M3-S12).
      setPending(await fetchApprovals(selected).catch(() => []));
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [selected]);

  useEffect(() => {
    void refresh();
  }, [refresh, tick]);
  useEffect(() => {
    void refreshSelected();
  }, [refreshSelected, tick]);

  // Handed over by Today, which is global and knows the project but not the view.
  const handled = useRef<number | null>(null);
  useEffect(() => {
    if (openRun !== null && handled.current !== openRun) {
      handled.current = openRun;
      setSelected(openRun);
      onOpenedRun();
    }
  }, [openRun, onOpenedRun]);

  // Esc closes the innermost thing this area owns, and nothing else - the shell handles
  // its own (M3-S11).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (expanded) setExpanded(false);
      else if (selected !== null) setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [expanded, selected]);

  return (
    <>
      <main className="main">
        {view === "work" && (
          <>
            <div className="main__header">
              <h2>Work</h2>
              {problem !== null && <span className="error">{problem}</span>}
            </div>

            <Prompt project={project.slug} onStarted={() => void refresh()} />

            <div className="main__header">
              <h2>Runs</h2>
            </div>

            {runs.length === 0 && (
              <p className="empty">
                Nothing has run here yet. Say what you want built and the team will plan it.
              </p>
            )}

            <div className="list">
              {runs.map((entry) => (
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

        {view === "board" && <Board project={project.slug} tick={tick} />}
        {view === "review" && <Review tick={tick} />}
        {view === "source" && <Source project={project.slug} node={null} />}
        {view === "team" && <Roster onChanged={onChanged} />}

        {view === "editor" && (
          <Suspense fallback={<p className="empty">Loading the editor…</p>}>
            <Editor project={project.slug} node={null} />
          </Suspense>
        )}

        {view === "terminal" && (
          <Suspense fallback={<p className="empty">Loading the terminal…</p>}>
            <TerminalPane project={project.slug} node={null} />
          </Suspense>
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
    </>
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
