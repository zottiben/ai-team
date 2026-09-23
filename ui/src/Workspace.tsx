import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";

import { AgentActivity } from "./AgentActivity";
import { Board } from "./Board";
import { Crew } from "./Crew";
import { Overview } from "./Overview";
import { PageLayout } from "./PageLayout";
import { Prompt, Seats } from "./Console";
import { Review } from "./Review";
import { Roster } from "./Roster";
import { Source } from "./Source";
import { Talk } from "./Talk";
import { workspaceTitle } from "./tree";
import {
  run as fetchRun,
  runEvents as fetchRunEvents,
  runs as fetchRuns,
  type Member,
  type Project,
  type Run,
  type RunDetail,
  type RunEvent,
  type Worktree,
} from "./api";

// Each is most of a megabyte that no other view needs (M4-S18, M4-S20).
const Editor = lazy(async () => ({ default: (await import("./Editor")).Editor }));
const TerminalPane = lazy(async () => ({
  default: (await import("./Terminal")).TerminalPane,
}));

/** The views that belong to one project (D18). */
const VIEWS = {
  overview: "Overview",
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
  workspace,
  view,
  tick,
  openRun,
  onOpenedRun,
  onChanged,
  onGo,
  onTeamStarted,
}: {
  project: Project;
  workspace: Worktree;
  view: WorkspaceView;
  tick: number;
  /** A run to open on arrival - how Today hands one over. */
  openRun: number | null;
  onOpenedRun: () => void;
  onChanged: () => void;
  /** Move to another view of this project - what the overview's panels link to. */
  onGo: (view: WorkspaceView) => void;
  /** Refresh the selected workspace after its team workflow starts. */
  onTeamStarted: () => void;
}) {
  const [runs, setRuns] = useState<Run[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // Talking to a seat and reading a run are both docks, and only one can be open: two
  // panels competing for the same edge is two things half-read.
  const [talking, setTalking] = useState<Member | null>(null);

  const refresh = useCallback(async () => {
    try {
      setRuns(await fetchRuns(project.id, workspace.path));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project.id, workspace.path]);

  // The selected run is refetched separately: it changes far more often than the list, and
  // re-reading everything on every tick would make the dock flicker.
  const refreshSelected = useCallback(async () => {
    if (selected === null) {
      setDetail(null);
      setEvents([]);
      return;
    }
    try {
      const [found, log] = await Promise.all([
        fetchRun(selected, workspace.path),
        fetchRunEvents(selected, undefined, workspace.path),
      ]);
      setDetail(found);
      setEvents(log);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [selected, workspace.path]);

  useEffect(() => {
    void refresh();
  }, [refresh, tick]);
  useEffect(() => {
    setSelected(null);
    setTalking(null);
  }, [workspace.path]);
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
      else if (talking !== null) setTalking(null);
      else if (selected !== null) setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [expanded, selected, talking]);

  // A pull request's worktree has a crew, not a run of its own: the orchestrator that
  // planned it works from the checkout above.
  const startWork =
    workspace.kind === "pr" ? (
      <section className="block">
        <div className="block__head">
          <h3>{workspace.slice_key}'s worktree</h3>
          <span className="faint">{workspace.plan}</span>
        </div>
        <p className="faint">
          One pull request lives here. Its seats build it task by task, and review comments on
          it come back here. A team run starts from the checkout above it.
        </p>
      </section>
    ) : (
      <section className="block">
        <div className="block__head">
          <h3>Start a team run</h3>
          <span className="faint">plan, approve, dispatch, verify</span>
        </div>
        <Prompt
          project={project.slug}
          workspace={workspace}
          onStarted={(runId) => {
            if (runId !== undefined) {
              setTalking(null);
              setSelected(runId);
            }
            void refresh();
            onTeamStarted();
          }}
        />
      </section>
    );

  return (
    <>
      <main className="main">
        {view === "overview" && (
          <Overview project={project} workspace={workspace} tick={tick} onGo={onGo} />
        )}

        {view === "work" && (
          <>
            <div className="main__header">
              <div>
                <h2>Work</h2>
                <span className="workspace-title mono">
                  {workspaceTitle(workspace)}
                </span>
              </div>
              {problem !== null && <span className="error">{problem}</span>}
            </div>

            <PageLayout
              view="work"
              panels={[
                {
                  id: "start",
                  label: "Start work",
                  span: 1,
                  content: startWork,
                },
                {
                  id: "activity",
                  label: "Agent activity",
                  span: 2,
                  content: (
                    <AgentActivity
                      runs={runs}
                      workspace={workspace.path}
                      tick={tick}
                      onOpenRun={(id) => {
                        setTalking(null);
                        setSelected(id);
                      }}
                    />
                  ),
                },
                {
                  id: "crew",
                  label: "Work graph",
                  span: 2,
                  content: (
                    <section className="block">
                      <Crew
                        project={project.slug}
                        workspace={workspace.path}
                        leaf={workspace.kind === "pr"}
                        tick={tick}
                        onOpenRun={(id) => {
                          setTalking(null);
                          setSelected(id);
                        }}
                        onTalk={(member) => {
                          setSelected(null);
                          setTalking(member);
                        }}
                        showGraph
                      />
                    </section>
                  ),
                },
                {
                  id: "runs",
                  label: "Runs",
                  span: 1,
                  content: (
                    <section className="block">
                      <div className="block__head">
                        <h3>Runs</h3>
                        <span className="faint">{runs.length} in this workspace</span>
                      </div>
                      {runs.length === 0 ? (
                        <p className="empty">
                          Nothing has run here yet. Start the team or talk directly to a maker.
                        </p>
                      ) : (
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
                      )}
                    </section>
                  ),
                },
              ]}
            />
          </>
        )}

        {view === "board" && (
          <Board project={project.slug} workspace={workspace.path} tick={tick} />
        )}
        {view === "review" && (
          <Review project={project.slug} workspace={workspace.path} tick={tick} />
        )}
        {view === "source" && (
          <Source project={project.slug} workspace={workspace.path} node={null} />
        )}
        {view === "team" && <Roster project={project.slug} onChanged={onChanged} />}

        {view === "editor" && (
          <Suspense fallback={<p className="empty">Loading the editor…</p>}>
            <Editor project={project.slug} workspace={workspace.path} node={null} />
          </Suspense>
        )}

        {view === "terminal" && (
          <Suspense fallback={<p className="empty">Loading the terminal…</p>}>
            <TerminalPane project={project.slug} workspace={workspace.path} node={null} />
          </Suspense>
        )}
      </main>

      {talking !== null && (
        <Talk
          member={talking}
          workspace={workspace.path}
          onClose={() => setTalking(null)}
          // A started turn shows up as a run, and an interrupted one changes what its seat
          // is doing - both of which the next tick would find anyway, but waiting a second
          // to see your own action land reads as nothing having happened.
          onSent={() => void refresh()}
        />
      )}

      {/* One surface at a time, and it collapses rather than covering what it describes. */}
      {detail !== null && talking === null && (
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
          <span>{event.message ?? event.summary}</span>
        </div>
      ))}
    </div>
  );
}
