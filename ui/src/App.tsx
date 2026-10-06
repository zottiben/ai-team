import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";

import { Analytics } from "./Analytics";
import { ChatView } from "./Chat";
import logo from "../../crates/ai-team-desktop/icons/mark.svg";
import { Notifications } from "./Notifications";
import { Popover } from "./Popover";
import { Projects } from "./Projects";
import { Roster } from "./Roster";
const Schedule = lazy(() => import("./Schedule").then(m => ({ default: m.Schedule })));
import { Settings } from "./Settings";
import { Setup } from "./Setup";
import { Today } from "./Today";
import {
  Workspace,
  WORKSPACE_VIEWS,
  workspaceViewName,
  type WorkspaceView,
} from "./Workspace";
import {
  doctor,
  projects as fetchProjects,
  run as fetchRun,
  subscribe,
  worktrees,
  type Project,
  type Worktree,
} from "./api";
import { chats as fetchChats, type Chat } from "./chat-api";
import { apply, followSystem, stored, type Theme } from "./theme";
import { nested, workspaceName } from "./tree";
import "./chat.css";

type Page =
  | "chat"
  | "projects"
  | "settings"
  | "schedule"
  | "today"
  | "analytics"
  | "team"
  | "tools"
  | "legacy";
const LAST_CHAT = "ai-team.last-chat";
const SIDEBAR_COLLAPSED = "ai-team.sidebar-collapsed";

function restored(): { project: string | null; chat: number | null } {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(LAST_CHAT) ?? "null",
    );
    if (
      value &&
      typeof value === "object" &&
      "project" in value &&
      "chat" in value &&
      typeof value.project === "string"
    ) {
      return {
        project: value.project,
        chat: typeof value.chat === "number" ? value.chat : null,
      };
    }
  } catch {
    /* An old or unavailable preference must not prevent opening the app. */
  }
  return { project: null, chat: null };
}

export default function App() {
  const [initial] = useState(restored);
  const [projects, setProjects] = useState<Project[]>([]);
  const [conversations, setConversations] = useState<Record<string, Chat[]>>(
    {},
  );
  const [projectSlug, setProjectSlug] = useState<string | null>(
    initial.project,
  );
  const [chatId, setChatId] = useState<number | null>(initial.chat);
  const [page, setPage] = useState<Page>("chat");
  const [tick, setTick] = useState(0);
  const [theme, setTheme] = useState<Theme>(stored);
  const [setup, setSetup] = useState(false);
  const interacted = useRef(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  const moreButton = useRef<HTMLButtonElement | null>(null);
  const [collapsed, setCollapsed] = useState(() => {
    try { return localStorage.getItem(SIDEBAR_COLLAPSED) === "true"; }
    catch { return false; }
  });
  useEffect(() => {
    try { localStorage.setItem(SIDEBAR_COLLAPSED, String(collapsed)); }
    catch { /* The sidebar still works without persistent storage. */ }
  }, [collapsed]);
  const [filter, setFilter] = useState("");
  const [trees, setTrees] = useState<Worktree[]>([]);
  const [workspace, setWorkspace] = useState<string | null>(null);
  const [tool, setTool] = useState<WorkspaceView>("source");
  const [legacyView, setLegacyView] = useState<WorkspaceView>("work");
  const [openRun, setOpenRun] = useState<number | null>(null);
  const [openReview, setOpenReview] = useState<number | null>(null);
  const toolNavigation = useRef(0);
  useEffect(
    () => () => {
      toolNavigation.current += 1;
    },
    [page, projectSlug, chatId, setup],
  );
  const generation = useRef(0);
  const checkedRestore = useRef(false);

  const changed = useCallback(() => setTick((value) => value + 1), []);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const list = await fetchProjects();
      const lists = await Promise.all(
        list.map(
          async (project) =>
            [project.slug, await fetchChats(project.slug)] as const,
        ),
      );
      if (request !== generation.current) return;
      setProjects(list);
      setConversations(Object.fromEntries(lists));
      setProjectSlug((current) =>
        list.some((project) => project.slug === current)
          ? current
          : (list[0]?.slug ?? null),
      );
      setProblem(null);
    } catch (error: unknown) {
      if (request === generation.current)
        setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh, tick]);
  useEffect(() => subscribe(changed), [changed]);
  useEffect(() => {
    let current = true;
    void doctor()
      .then((report) => {
        if (current && !interacted.current) setSetup(report.needs_setup);
      })
      .catch((error: unknown) => {
        if (current)
          setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, []);
  useEffect(() => {
    apply(theme);
    return followSystem(() => theme);
  }, [theme]);
  useEffect(() => {
    if (!projectSlug) return;
    try {
      localStorage.setItem(
        LAST_CHAT,
        JSON.stringify({ project: projectSlug, chat: chatId }),
      );
    } catch {
      /* Navigation remains usable when storage is disabled. */
    }
  }, [projectSlug, chatId]);
  useEffect(() => {
    const current = projectSlug ? conversations[projectSlug] : undefined;
    if (!current || checkedRestore.current) return;
    checkedRestore.current = true;
    if (chatId !== null && !current.some((chat) => chat.id === chatId))
      setChatId(null);
  }, [conversations, projectSlug, chatId]);
  useEffect(() => {
    if ((page !== "tools" && page !== "legacy") || !projectSlug) return;
    let current = true;
    void worktrees(projectSlug)
      .then((found) => {
        if (!current) return;
        setTrees(found);
        setWorkspace((path) =>
          found.some((tree) => tree.path === path)
            ? path
            : (found.find((tree) => tree.main)?.path ?? found[0]?.path ?? null),
        );
      })
      .catch((error: unknown) => {
        if (current)
          setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [projectSlug, page, tick]);

  const project = projects.find((entry) => entry.slug === projectSlug);
  const selectedTree = trees.find((tree) => tree.path === workspace);
  const navigate = (slug: string, id: number | null) => {
    setProjectSlug(slug);
    setChatId(id);
    setPage("chat");
    setSetup(false);
  };
  const newChat = () => {
    if (projectSlug) navigate(projectSlug, null);
    else setPage("projects");
  };
  const openExecution = async (
    run: number | null,
    slug: string | null,
    review: number | null = null,
  ) => {
    const target = slug ?? projectSlug;
    if (!target) return;
    const request = ++toolNavigation.current;
    try {
      const detail = run === null ? null : await fetchRun(run);
      if (request !== toolNavigation.current) return;
      setProjectSlug(target);
      setWorkspace(detail?.workspace_path ?? null);
      setOpenRun(review === null ? run : null);
      setOpenReview(review);
      setLegacyView(review === null ? "work" : "review");
      setPage("legacy");
    } catch (error) {
      if (request === toolNavigation.current)
        setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  const notificationButton = <Notifications tick={tick} onOpen={notification => {
    const owner = projects.find(candidate => candidate.id === notification.project_id);
    if (!owner) {
      setProblem("This notification's project is not available. Restore its registration in Projects to inspect it.");
      return;
    }
    if (notification.chat_id != null) {
      navigate(owner.slug, notification.chat_id);
      return;
    }
    setProjectSlug(owner.slug);
    setWorkspace(notification.workspace_path);
    setOpenRun(notification.run_id);
    setLegacyView("work");
    setPage("legacy");
    setSetup(false);
  }} />;

  return (
    <div className="chat-shell" data-sidebar-collapsed={collapsed} onClickCapture={() => { interacted.current = true; }} onKeyDownCapture={() => { interacted.current = true; }}>
      <nav className="app-rail" aria-label="Application">
        <button type="button" aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"} title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-expanded={!collapsed} aria-controls="chat-sidebar" onClick={() => setCollapsed(value => !value)}>
          <Icon name={collapsed ? "expand" : "collapse"} />
        </button>
        {collapsed && <button type="button" aria-label="New chat" title="New chat" onClick={newChat}><Icon name="edit" /></button>}
        {(
          [
            ["chat", "Chats", "chat"],
            ["projects", "Projects", "folder"],
            ["schedule", "Schedule", "clock"],
          ] as const
        ).map(([target, label, icon]) => (
          <button
            key={target}
            type="button"
            title={label}
            aria-label={label}
            aria-current={!setup && page === target ? "page" : undefined}
            onClick={() => {
              setPage(target);
              setSetup(false);
            }}
          >
            <Icon name={icon} />
          </button>
        ))}
        {collapsed && notificationButton}
        <button
          ref={moreButton}
          type="button"
          aria-label="More tools"
          title="More tools"
          aria-expanded={more}
          onClick={() => setMore(!more)}
        >
          <Icon name="more" />
        </button>
        <div className="app-rail-bottom">
          <button
            type="button"
            aria-label="Setup"
            title="Setup"
            onClick={() => setSetup(true)}
          >
            <Icon name="help" />
          </button>
          <button
            type="button"
            title="Settings"
            aria-label="Settings"
            aria-current={!setup && page === "settings" ? "page" : undefined}
            onClick={() => {
              setPage("settings");
              setSetup(false);
            }}
          >
            <Icon name="settings" />
          </button>
        </div>
      </nav>
      <aside id="chat-sidebar" className="chat-sidebar" hidden={collapsed}>
        <div className="chat-brand">
          <img src={logo} width="28" height="28" alt="AI Team logo" />
          <strong>AI Team</strong>
          {!collapsed && notificationButton}
        </div>
        <button type="button" className="chat-new" onClick={newChat}>
          <Icon name="edit" />
          New chat
        </button>
        <input
          className="chat-search"
          aria-label="Find chats"
          placeholder="Find a chat…"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <div
          className="chat-projects"
          role="navigation"
          aria-label="Projects and chats"
        >
          <div className="chat-section-heading">
            <span>Projects</span>
            <button
              type="button"
              aria-label="Add project"
              title="Add project"
              onClick={() => {
                setPage("projects");
                setSetup(false);
              }}
            >
              +
            </button>
          </div>
          {projects.map((entry) => (
            <div className="chat-project" key={entry.id}>
              <button
                type="button"
                className="chat-project-name"
                aria-current={
                  page === "chat" &&
                  entry.slug === projectSlug &&
                  chatId === null
                    ? "page"
                    : undefined
                }
                onClick={() => navigate(entry.slug, null)}
              >
                <Icon name="folder" />
                <span>{entry.name}</span>
              </button>
              <div
                className="chat-thread-list"
                aria-label={`${entry.name} chats`}
              >
                {(conversations[entry.slug] ?? [])
                  .filter((chat) =>
                    chat.title.toLowerCase().includes(filter.toLowerCase()),
                  )
                  .map((chat) => (
                    <button
                      type="button"
                      key={chat.id}
                      title={chat.title}
                      aria-current={
                        page === "chat" &&
                        projectSlug === entry.slug &&
                        chatId === chat.id
                          ? "page"
                          : undefined
                      }
                      onClick={() => navigate(entry.slug, chat.id)}
                    >
                      <span>{chat.title}</span>
                      {chat.active_node_id !== null && (
                        <span className="chat-pulse" aria-label="Active turn" />
                      )}
                    </button>
                  ))}
                {conversations[entry.slug]?.length === 0 && (
                  <span className="faint">No chats yet</span>
                )}
              </div>
            </div>
          ))}
          {projects.length === 0 && (
            <p className="faint">Add a project to start a conversation.</p>
          )}
        </div>
        {more && <Popover anchor={moreButton} label="Additional tools" width={220} onClose={() => setMore(false)}>
          <nav className="chat-secondary" aria-label="More tools">
            {(
              [
                ["today", "Today"],
                ["analytics", "Analytics"],
                ["team", "Default team"],
                ["tools", "Project tools"],
              ] as const
            ).map(([target, label]) => (
              <button
                type="button"
                key={target}
                className="nav-item"
                onClick={() => {
                  setPage(target);
                  setSetup(false);
                  setMore(false);
                }}
              >
                {label}
              </button>
            ))}
          </nav>
        </Popover>}
        <div className="chat-sidebar-footer">
          <span>Powered by Pi</span>
          {project && (
            <button
              className="button"
              onClick={() => {
                setPage("tools");
                setSetup(false);
              }}
            >
              Project tools
            </button>
          )}
        </div>
      </aside>
      <div className="chat-main">
        {problem && (
          <p className="error chat-notice" role="alert">
            {problem}
          </p>
        )}
        {setup ? (
          <main className="main">
            <Setup
              onProjects={() => { setSetup(false); setPage("projects"); changed(); }}
              onReady={() => {
                setSetup(false);
                changed();
              }}
            />
          </main>
        ) : page === "chat" ? (
          project ? (
            <ChatView
              key={`${project.slug}:${chatId ?? "new"}`}
              id={chatId}
              project={project}
              tick={tick}
              onCreated={(id) => {
                setChatId(id);
                changed();
              }}
              onChanged={changed}
              onArchived={() => {
                setChatId(null);
                changed();
              }}
              onSettings={() => setPage("settings")}
            />
          ) : (
            <main className="chat-welcome">
              <h1>What should we build?</h1>
              <p>Choose a project. Start with one agent.</p>
              <button
                className="button button--primary"
                onClick={() => setPage("projects")}
              >
                Add a project
              </button>
            </main>
          )
        ) : page === "tools" || page === "legacy" ? (
          project ? (
            <>
              {page === "legacy" ? <nav className="project-tool-tabs" aria-label="Legacy execution">
                <button className="button" onClick={() => setPage("today")}>Back to Today</button>
                <span className="faint">Legacy execution · {project.name} · {workspaceViewName(legacyView)}</span>
              </nav> : <nav className="project-tool-tabs" aria-label="Project tools">
                <select
                  aria-label="Project checkout"
                  value={workspace ?? ""}
                  onChange={(event) => setWorkspace(event.target.value)}
                >
                  {nested(trees).map(({ tree, depth }) => (
                    <option key={tree.path} value={tree.path}>
                      {"— ".repeat(depth)}
                      {workspaceName(tree)}
                    </option>
                  ))}
                </select>
                {WORKSPACE_VIEWS.map((view) => (
                  <button
                    className="button"
                    key={view}
                    aria-pressed={view === tool}
                    onClick={() => setTool(view)}
                  >
                    {workspaceViewName(view)}
                  </button>
                ))}
              </nav>}
              {selectedTree ? (
                <Workspace
                  project={project}
                  workspace={selectedTree}
                  view={page === "legacy" ? legacyView : tool}
                  tick={tick}
                  openRun={openRun}
                  onOpenedRun={() => setOpenRun(null)}
                  openReview={openReview}
                  onOpenedReview={() => setOpenReview(null)}
                  onChanged={changed}
                  onGo={page === "legacy" ? setLegacyView : setTool}
                  onTeamStarted={changed}
                />
              ) : (
                <p className="empty">Reading project checkouts…</p>
              )}
            </>
          ) : (
            <p className="empty">Add a project to use its tools.</p>
          )
        ) : (
          <main className="main">
            {page === "projects" && <Projects onChanged={changed} />}
            {page === "settings" && (
              <Settings theme={theme} onTheme={setTheme} onChanged={changed} />
            )}
            {page === "schedule" && (
              <Suspense fallback={<p>Loading schedules…</p>}><Schedule
                tick={tick}
                // Scheduled work happens in one exact chat, so its result opens there
                // rather than in whichever run the project last had.
                onOpenChat={(slug, id) => navigate(slug, id)}
              /></Suspense>
            )}
            {page === "team" && <Roster onChanged={changed} />}
            {page === "analytics" && <Analytics tick={tick} />}
            {page === "today" && (
              <Today
                tick={tick}
                onOpenRun={(id, slug) => void openExecution(id, slug)}
                onOpenReview={(id, slug, run) =>
                  void openExecution(run, slug, id)
                }
              />
            )}
          </main>
        )}
      </div>
    </div>
  );
}

function Icon({
  name,
}: {
  name: "chat" | "folder" | "clock" | "more" | "help" | "settings" | "edit" | "collapse" | "expand";
}) {
  const paths = {
    collapse: "M3 4h18v16H3V4ZM8 4v16m8-12-4 4 4 4",
    expand: "M3 4h18v16H3V4ZM8 4v16m4-12 4 4-4 4",
    chat: "M4 4h16v12H9l-5 4V4Z",
    folder: "M3 6h7l2 2h9v12H3V6Z",
    clock: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18M12 7v6l4 2",
    more: "M5 12h1M11 12h1M17 12h1",
    help: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18M9 9a3 3 0 1 1 5 2c-2 1-2 2-2 3M12 17h.01",
    settings: "M4 7h16M4 17h16M8 4v6M16 14v6",
    edit: "M14 4H4v16h16V10M10 14l2-5 6-6 3 3-6 6-5 2Z",
  };
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}
