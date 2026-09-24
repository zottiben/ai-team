import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";

import { Analytics } from "./Analytics";
import { Notifications } from "./Notifications";
import { Projects } from "./Projects";
import { Roster } from "./Roster";
import { Schedule } from "./Schedule";
import { Settings } from "./Settings";
import { HealthBanner, Setup } from "./Setup";
import { Today } from "./Today";
import { UpdateBanner } from "./Update";
import {
  Workspace,
  WORKSPACE_VIEWS,
  workspaceViewName,
  type WorkspaceView,
} from "./Workspace";
import {
  doctor as doctorReport,
  health,
  projects as fetchProjects,
  run as fetchRun,
  subscribe,
  worktrees as fetchWorktrees,
  type Health,
  type Notification as TeamNotification,
  type Project,
  type Worktree,
} from "./api";
import { apply, followSystem, stored, type Theme } from "./theme";
import { nested, workspaceDetail, workspaceName } from "./tree";

/**
 * The shell, and the two levels the window has (D18).
 *
 * The views named here answer questions *across* projects, and none of them takes one -
 * "what should I do now" and "which pairing earns its seat" are not questions about a
 * repository. Selecting a project hands over to [`Workspace`], which owns everything about
 * that one.
 *
 * It used to be a flat list of twelve views each taking a project as a filter, which made
 * every one answer a slightly different question depending on a selection elsewhere, and
 * made the thing somebody actually does - sit in one repository and run a team - no more
 * prominent than the schedule.
 */
const GLOBAL_VIEWS = {
  today: "Today",
  analytics: "Analytics",
  schedule: "Schedule",
  projects: "Projects",
  team: "Default team",
  settings: "Settings",
} as const;

type GlobalView = keyof typeof GLOBAL_VIEWS;

/** Where you are: across everything, or inside one project. */
type Place =
  | { level: "global"; view: GlobalView }
  | { level: "project"; slug: string; workspace: string | null; view: WorkspaceView };

export default function App() {
  const [info, setInfo] = useState<Health | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [workspaces, setWorkspaces] = useState<Record<string, Worktree[]>>({});
  const [place, setPlace] = useState<Place>({ level: "global", view: "today" });
  const [setupOpen, setSetupOpen] = useState(false);
  const [overlay, setOverlay] = useState<null | "about">(null);
  const [theme, setTheme] = useState<Theme>(stored);
  const [problem, setProblem] = useState<string | null>(null);
  // Bumped on every server tick, so views re-read without each owning a subscription.
  const [tick, setTick] = useState(0);
  // Bumped wherever the window changes the machine's configuration - seats, providers,
  // projects, a repair in Setup - so the health banner re-reads it then, not per event.
  const [readiness, setReadiness] = useState(0);
  const configured = useCallback(() => {
    setReadiness((value) => value + 1);
    setTick((value) => value + 1);
  }, []);
  // Leaving Setup, however it closes, is leaving the page repairs are made on.
  const wasSetupOpen = useRef(false);
  useEffect(() => {
    if (wasSetupOpen.current && !setupOpen) setReadiness((value) => value + 1);
    wasSetupOpen.current = setupOpen;
  }, [setupOpen]);
  // A run Today asked for, carried until the workspace has taken it.
  const [openRun, setOpenRun] = useState<number | null>(null);
  const [firstRun, setFirstRun] = useState<boolean | null>(null);
  // Where you last were in each project. A command centre you come back to should be where
  // you left it - resetting to Work every time makes returning feel like starting over.
  const [lastView, setLastView] = useState<Record<string, WorkspaceView>>({});
  const [lastWorkspace, setLastWorkspace] = useState<Record<string, string | null>>({});

  const refresh = useCallback(async () => {
    try {
      setProjects(await fetchProjects());
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void health().then(setInfo).catch(() => {});
    void refresh();
  }, [refresh]);

  const loadWorkspaces = useCallback(async (slug: string) => {
    try {
      const found = await fetchWorktrees(slug);
      setWorkspaces((current) => ({ ...current, [slug]: found }));
    } catch {
      // The project still opens. Its surfaces will report the precise checkout error;
      // the sidebar does not replace the whole window with it.
      setWorkspaces((current) => ({ ...current, [slug]: [] }));
    }
  }, []);

  useEffect(() => {
    for (const project of projects) {
      if (workspaces[project.slug] === undefined) void loadWorkspaces(project.slug);
    }
  }, [loadWorkspaces, projects, workspaces]);

  useEffect(() => {
    apply(theme);
    return followSystem(() => theme);
  }, [theme]);

  // Asked once, on mount. A machine that needs setting up opens on setup rather than on an
  // empty Today, which is the confusing first impression this replaces (M6-S27).
  useEffect(() => {
    void doctorReport()
      .then((report) => {
        setFirstRun(report.needs_setup);
        if (report.needs_setup) setSetupOpen(true);
      })
      .catch(() => setFirstRun(false));
  }, []);

  // One subscription for the whole window: `ait ui` and `ait run` are separate processes
  // sharing a SQLite file, so the tick says the database changed and each view re-reads
  // whatever it is showing (M3-S11).
  useEffect(() => {
    return subscribe(() => {
      void refresh();
      setTick((value) => value + 1);
    });
  }, [refresh]);

  // Esc closes what the shell owns. The workspace handles its own, innermost first.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (overlay !== null) setOverlay(null);
      else if (setupOpen) setSetupOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [overlay, setupOpen]);

  const inside = place.level === "project" ? projects.find((p) => p.slug === place.slug) : undefined;
  const insideWorkspace =
    inside === undefined || place.level !== "project"
      ? undefined
      : place.workspace === null
        ? workspaces[inside.slug]?.find((workspace) => workspace.main)
        : workspaces[inside.slug]?.find((workspace) => workspace.path === place.workspace);

  const openNotification = useCallback(
    (notification: TeamNotification) => {
      const project = projects.find((entry) => entry.id === notification.project_id);
      if (project === undefined) return;
      if (notification.run_id !== null) setOpenRun(notification.run_id);
      setLastWorkspace((seen) => ({
        ...seen,
        [project.slug]: notification.workspace_path,
      }));
      setLastView((seen) => ({ ...seen, [project.slug]: "work" }));
      setPlace({
        level: "project",
        slug: project.slug,
        workspace: notification.workspace_path,
        view: "work",
      });
    },
    [projects],
  );

  // Worktree state changes much less often than events. Polling `awt` on every event tick
  // would spawn two processes hundreds of times during a turn, so refresh only the active
  // repository on a human-scale cadence.
  useEffect(() => {
    if (inside === undefined) return undefined;
    const timer = setInterval(() => void loadWorkspaces(inside.slug), 5000);
    return () => clearInterval(timer);
  }, [inside, loadWorkspaces]);

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="sidebar__brand">
          <h1>ai-team</h1>
          <span className="sidebar__version">{info?.version ?? ""}</span>
        </div>

        {/* Inside a project, its own navigation replaces the global list rather than sitting
            beside it: two lists of views is two places to look for the same thing. */}
        {inside === undefined ? (
          <nav className="sidebar__section" aria-label="Everything">
            <span className="sidebar__label">Everything</span>
            {(Object.keys(GLOBAL_VIEWS) as GlobalView[]).map((option) => (
              <button
                type="button"
                key={option}
                className="nav-item"
                aria-current={place.level === "global" && place.view === option}
                onClick={() => setPlace({ level: "global", view: option })}
              >
                <span>{GLOBAL_VIEWS[option]}</span>
              </button>
            ))}
          </nav>
        ) : (
          <nav className="sidebar__section" aria-label={inside.name}>
            <button
              type="button"
              className="nav-item sidebar__back"
              onClick={() => setPlace({ level: "global", view: "today" })}
            >
              <span>← Everything</span>
            </button>
            <span className="sidebar__label">
              {inside.name}
              {insideWorkspace !== undefined && (
                <span className="sidebar__workspace-label mono">
                  {workspaceName(insideWorkspace)}
                </span>
              )}
            </span>
            {WORKSPACE_VIEWS.map((option) => (
              <button
                type="button"
                key={option}
                className="nav-item"
                aria-current={place.level === "project" && place.view === option}
                onClick={() => {
                  setLastView((seen) => ({ ...seen, [inside.slug]: option }));
                  setPlace({
                    level: "project",
                    slug: inside.slug,
                    workspace: place.level === "project" ? place.workspace : null,
                    view: option,
                  });
                }}
              >
                <span>{workspaceViewName(option)}</span>
              </button>
            ))}
          </nav>
        )}

        <nav className="sidebar__section" aria-label="Projects">
          <span className="sidebar__label">Projects</span>
          {projects.map((entry) => {
            const trees = workspaces[entry.slug];
            const currentPath =
              place.level === "project" && place.slug === entry.slug ? place.workspace : undefined;
            return (
              <div className="sidebar-project" key={entry.id}>
                <button
                  type="button"
                  className="nav-item sidebar-project__repo"
                  aria-current={inside?.id === entry.id}
                  onClick={() => {
                    const workspace =
                      lastWorkspace[entry.slug] ?? trees?.find((tree) => tree.main)?.path ?? null;
                    setPlace({
                      level: "project",
                      slug: entry.slug,
                      workspace,
                      view: lastView[entry.slug] ?? "overview",
                    });
                  }}
                >
                  <span>{entry.name}</span>
                  <span className="nav-item__count">
                    {entry.open_runs > 0 ? entry.open_runs : ""}
                  </span>
                </button>
                <div className="sidebar-project__workspaces" aria-label={`${entry.name} workspaces`}>
                  {trees === undefined && <span className="faint">reading checkouts…</span>}
                  {trees !== undefined &&
                    nested(trees).map(({ tree: workspace, depth }) => {
                    const selected =
                      inside?.id === entry.id &&
                      (currentPath === workspace.path || (currentPath === null && workspace.main));
                    const detail = workspaceDetail(workspace);
                    return (
                      <button
                        type="button"
                        key={workspace.path}
                        className="nav-item nav-item--workspace"
                        aria-current={selected}
                        title={workspace.path}
                        style={{ "--workspace-depth": depth } as CSSProperties}
                        onClick={() => {
                          setLastWorkspace((seen) => ({
                            ...seen,
                            [entry.slug]: workspace.path,
                          }));
                          setPlace({
                            level: "project",
                            slug: entry.slug,
                            workspace: workspace.path,
                            view: lastView[entry.slug] ?? "overview",
                          });
                        }}
                      >
                        <span className="workspace-dot" data-status={workspace.status} />
                        <span className="nav-item__name">{workspaceName(workspace)}</span>
                        {detail !== null && <span className="nav-item__detail">{detail}</span>}
                        {/* A pull request's worktree is always leased, so saying so is noise; a
                            lease left behind by a reboot is not. */}
                        {workspace.lease_holder !== null &&
                          (workspace.kind !== "pr" ||
                            workspace.lease_holder.startsWith("orphaned:")) && (
                            <span className="nav-item__lease" title={workspace.lease_holder}>
                              {workspace.lease_holder.startsWith("orphaned:") ? "!" : "leased"}
                            </span>
                          )}
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })}
          {projects.length === 0 && (
            <span className="faint">None yet - add one from Projects.</span>
          )}
        </nav>

        <div className="sidebar__section" style={{ marginTop: "auto" }}>
          <Notifications tick={tick} onOpen={openNotification} />
          {/* Visible from every page, because a machine that cannot run anything is worth
              interrupting whatever somebody is looking at. */}
          <HealthBanner recheck={readiness} onOpen={() => setSetupOpen(true)} />
          {firstRun === true && !setupOpen && (
            <button type="button" className="nav-item" onClick={() => setSetupOpen(true)}>
              <span>Finish setting up</span>
            </button>
          )}
          <UpdateBanner />
          <button type="button" className="nav-item" onClick={() => setOverlay("about")}>
            <span>About</span>
            <span className="kbd">Esc</span>
          </button>
        </div>
      </aside>

      {/* Setup is a full-window state rather than a view, because it is what you are doing
          rather than somewhere you are - and it has to be reachable from inside a project
          as well as from the global list. */}
      {setupOpen ? (
        <main className="main">
          <Setup
            onReady={() => {
              setFirstRun(false);
              setSetupOpen(false);
              void refresh();
              setTick((value) => value + 1);
            }}
          />
        </main>
      ) : inside !== undefined && place.level === "project" ? (
        insideWorkspace === undefined ? (
          <main className="main">
            <p className="empty">Reading this repository's workspaces…</p>
          </main>
        ) : (
          <Workspace
            project={inside}
            workspace={insideWorkspace}
            view={place.view}
            tick={tick}
            openRun={openRun}
            onOpenedRun={() => setOpenRun(null)}
            onChanged={() => {
              void loadWorkspaces(inside.slug);
              configured();
            }}
            onGo={(view) => {
              setLastView((seen) => ({ ...seen, [inside.slug]: view }));
              setPlace({
                level: "project",
                slug: inside.slug,
                workspace: place.workspace,
                view,
              });
            }}
            onTeamStarted={() => setTick((value) => value + 1)}
          />
        )
      ) : (
        <main className="main">
          {problem !== null && <p className="error">{problem}</p>}

          {place.level === "global" && place.view === "today" && (
            <Today
              tick={tick}
              // Today spans projects, so following an item has to say which one - it enters
              // that project and opens the run there, rather than rendering a run in a
              // second place that would drift from the first.
              onOpenRun={(id, slug) => {
                setOpenRun(id);
                const target = slug ?? projects[0]?.slug;
                if (target === undefined) return;
                void fetchRun(id)
                  .then((detail) => {
                    const workspace =
                      detail.workspace_path ??
                      detail.nodes.find((node) => node.worktree_path !== null)?.worktree_path ??
                      null;
                    setLastWorkspace((seen) => ({ ...seen, [target]: workspace }));
                    setPlace({ level: "project", slug: target, workspace, view: "work" });
                  })
                  .catch(() => {
                    setPlace({ level: "project", slug: target, workspace: null, view: "work" });
                  });
              }}
            />
          )}

          {place.level === "global" && place.view === "analytics" && <Analytics tick={tick} />}

          {place.level === "global" && place.view === "schedule" && <Schedule tick={tick} />}

          {place.level === "global" && place.view === "projects" && (
            <Projects
              onChanged={() => {
                void refresh();
                configured();
              }}
            />
          )}

          {place.level === "global" && place.view === "team" && (
            <Roster onChanged={configured} />
          )}

          {place.level === "global" && place.view === "settings" && (
            <Settings
              theme={theme}
              onTheme={setTheme}
              onChanged={configured}
            />
          )}
        </main>
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
            <button
              type="button"
              className="button button--primary"
              onClick={() => setOverlay(null)}
            >
              Close
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
