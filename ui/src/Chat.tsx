import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { BoardMarkdown } from "./BoardMarkdown";
import { ChatPlanning } from "./ChatPlan";
import { ChatTeam, TEAM_PHASES } from "./ChatTeam";
import { setChatMode } from "./team-api";
import { models, type ModelChoice, type Project, type RunEvent } from "./api";
import {
  archiveChat,
  chat,
  chatEvents,
  createChat,
  renameChat,
  resumeChat,
  sendChat,
  stopChat,
  type ChatDetail,
} from "./chat-api";

const LABELS = {
  empty: "Ready",
  idle: "Ready",
  running: "Working",
  stopping: "Stopping…",
  interrupted: "Interrupted",
  failed: "Needs attention",
  stopped: "Stopped",
  awaiting_approval: "Awaiting approval",
  team_blocked: "Team needs attention",
  team_interrupted: "Team needs recovery",
};

export function ChatView({
  id,
  project,
  tick,
  onCreated,
  onChanged,
  onArchived,
  onSettings,
}: {
  id: number | null;
  project: Project;
  tick: number;
  onCreated: (id: number) => void;
  onChanged: () => void;
  onArchived: () => void;
  onSettings: () => void;
}) {
  const [detail, setDetail] = useState<ChatDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [view, setView] = useState<"chat" | "overview">("chat");
  const [catalogue, setCatalogue] = useState<ModelChoice[]>([]);
  const [modelKey, setModelKey] = useState("");
  const [newMode, setNewMode] = useState<"single" | "team">("single");
  const [modelError, setModelError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [editingTitle, setEditingTitle] = useState(false);
  const [title, setTitle] = useState("");
  const [following, setFollowing] = useState(true);
  const [clock, setClock] = useState(Date.now());
  const alive = useRef(true);
  const loading = useRef(false);
  const pending = useRef(false);
  const cursor = useRef(0);
  const created = useRef<number | null>(null);
  const submission = useRef<{ body: string; key: string } | null>(null);
  const feed = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = useCallback(async () => {
    if (id === null) return;
    if (loading.current) {
      pending.current = true;
      return;
    }
    loading.current = true;
    try {
      do {
        pending.current = false;
        let after = cursor.current;
        const added: RunEvent[] = [];
        while (alive.current) {
          const page = await chatEvents(id, after);
          added.push(...page);
          after = page.at(-1)?.id ?? after;
          if (page.length < 500) break;
        }
        const found = await chat(id);
        if (!alive.current) return;
        cursor.current = after;
        setEvents((previous) => [...previous, ...added]);
        setDetail(found);
      } while (pending.current && alive.current);
    } catch (error: unknown) {
      if (alive.current)
        setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      loading.current = false;
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load, tick]);
  useEffect(() => {
    if (id !== null) return;
    let current = true;
    void models()
      .then((found) => {
        if (!current) return;
        setCatalogue(found.models);
        setModelError(found.error);
        setModelKey(
          (previous) =>
            previous ||
            (found.models[0]
              ? `${found.models[0].provider}/${found.models[0].model}`
              : ""),
        );
      })
      .catch((error: unknown) => {
        if (current)
          setModelError(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [id, tick]);

  const active = detail?.active_node_id != null;
  const running = detail?.state === "running" || detail?.state === "stopping";
  const latest = detail?.turns.at(-1);
  const activeTeam = detail?.turns.find((turn) => turn.team?.control_node_id === detail.active_node_id)?.team;
  const mode = detail?.mode ?? newMode;
  const currentEvents = events.filter(
    (event) =>
      event.node_run_id === (detail?.active_node_id ?? latest?.node.id),
  );
  const lastActivity = currentEvents.at(-1);
  const started = latest?.node.started_at;
  const elapsed = started
    ? Math.max(
        0,
        Math.floor(
          (clock -
            Date.parse(started.endsWith("Z") ? started : `${started}Z`)) /
            1000,
        ),
      )
    : 0;

  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setClock(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);

  useLayoutEffect(() => {
    if (following && feed.current)
      feed.current.scrollTop = feed.current.scrollHeight;
  }, [following, events.length, detail?.live_text, view]);

  const send = async () => {
    if (busy || active || !message.trim() || (id !== null && detail === null))
      return;
    const chosen = catalogue.find(
      (choice) => `${choice.provider}/${choice.model}` === modelKey,
    );
    if (id === null && !chosen) return;
    setBusy(true);
    setProblem(null);
    const body = message.trim();
    if (submission.current?.body !== body)
      submission.current = { body, key: crypto.randomUUID() };
    try {
      let target = id ?? created.current;
      if (target === null && chosen) {
        const draft = await createChat({
          project: project.slug,
          provider: chosen.provider,
          model: chosen.model,
          reasoning: "high",
          ...(newMode === "team" ? { mode: newMode } : {}),
        });
        target = draft.id;
        created.current = target;
      }
      if (target === null) return;
      await sendChat(target, body, submission.current.key);
      if (!alive.current) return;
      setMessage("");
      submission.current = null;
      setFollowing(true);
      onChanged();
      if (id === null) onCreated(target);
      else await load();
    } catch (error: unknown) {
      if (alive.current)
        setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  const command = async (act: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setProblem(null);
    try {
      await act();
      if (!alive.current) return;
      onChanged();
    } catch (error: unknown) {
      if (alive.current)
        setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      // A failed close/recovery may still have recorded irreversible intent.
      if (alive.current) {
        await load();
        setBusy(false);
      }
    }
  };

  const controls =
    detail?.active_node_id == null || activeTeam ? null : (
      <div className="chat-controls">
        {detail.can_resume && (
          <button
            className="button button--primary"
            disabled={busy}
            onClick={() =>
              void command(() => resumeChat(detail.id, detail.active_node_id!))
            }
          >
            Resume turn
          </button>
        )}
        <button
          className="button"
          disabled={busy || detail.stop_requested || detail.orphan_running}
          onClick={() =>
            void command(() => stopChat(detail.id, detail.active_node_id!))
          }
        >
          {detail.stop_requested ? "Stopping…" : "Stop turn"}
        </button>
      </div>
    );

  return (
    <main
      className="chat-workspace"
      aria-label={`${project.name} conversation`}
    >
      <header className="chat-header">
        <div className="chat-heading">
          <span className="faint">{project.name}</span>
          {editingTitle && detail ? (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void command(async () => {
                  await renameChat(detail.id, title);
                  setEditingTitle(false);
                });
              }}
            >
              <input
                aria-label="Chat title"
                autoFocus
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                maxLength={160}
              />
              <button className="button" disabled={busy}>
                Save title
              </button>
              <button
                className="button"
                type="button"
                onClick={() => setEditingTitle(false)}
              >
                Cancel
              </button>
            </form>
          ) : (
            <h1>
              {detail?.title ?? (id === null ? "New chat" : "Loading chat…")}
            </h1>
          )}
        </div>
        {detail && (
          <div className="chat-header-actions">
            <button
              className="button"
              onClick={() => {
                setTitle(detail.title);
                setEditingTitle(true);
              }}
            >
              Rename
            </button>
            <button
              className="button"
              disabled={busy || active}
              onClick={() =>
                void command(async () => {
                  await archiveChat(detail.id);
                  if (alive.current) onArchived();
                })
              }
            >
              Archive
            </button>
          </div>
        )}
      </header>
      <div className="chat-tabs" role="tablist" aria-label="Chat views">
        {(["chat", "overview"] as const).map((tab) => (
          <button
            key={tab}
            id={`chat-tab-${tab}`}
            role="tab"
            aria-selected={view === tab}
            aria-controls="chat-panel"
            tabIndex={view === tab ? 0 : -1}
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
                const next = tab === "chat" ? "overview" : "chat";
                setView(next);
                document.getElementById(`chat-tab-${next}`)?.focus();
              }
            }}
            onClick={() => setView(tab)}
          >
            {tab === "chat" ? "Chat" : "Overview"}
          </button>
        ))}
        <span
          className="chat-state"
          role="status"
          data-state={detail?.state ?? "empty"}
        >
          {LABELS[detail?.state ?? "empty"]}
        </span>
      </div>
      {detail?.recovery_error && (
        <p className="error chat-notice" role="alert">
          Initial recovery could not inspect interrupted work: {detail.recovery_error}.
          {" "}No work was resumed. Restart ai-team to retry automatic recovery.
        </p>
      )}
      {problem && (
        <p className="error chat-notice" role="alert">
          {problem}
        </p>
      )}
      {detail?.state === "interrupted" && (
        <div className="chat-notice notice">
          <strong>This turn was interrupted.</strong> Your history and working
          files are kept.
          {detail.orphan_running
            ? " The original Pi process is still running. It must exit before another turn can use this checkout."
            : " Resume it, or stop it and send a new instruction."}
          {controls}
        </div>
      )}
      {activeTeam && view === "chat" && <div className="chat-notice notice">
        <strong>{TEAM_PHASES[activeTeam.phase]}.</strong> {activeTeam.reason}
        <button className="button" onClick={() => setView("overview")}>Open team controls</button>
      </div>}
      {latest?.node.blocked_reason && !active && (
        <p className="chat-notice notice">{latest.node.blocked_reason}</p>
      )}
      <div
        id="chat-panel"
        role="tabpanel"
        aria-labelledby={`chat-tab-${view}`}
        className="chat-panel"
      >
        {view === "overview" ? (
          <div className="chat-overview">
            <h2>This conversation</h2>
            <div className="chat-overview-grid">
              <section className="chat-overview-card">
                <h3>Execution</h3>
                <strong>{LABELS[detail?.state ?? "empty"]}</strong>
                <p>{activeTeam ? "Pi team · chat-owned draft worktrees" : "Single Pi agent · local checkout"}</p>
                {running && (
                  <>
                    <div
                      className="chat-live-meter"
                      role="progressbar"
                      aria-label="Agent working"
                    />
                    <p className="mono">
                      {Number.isFinite(elapsed) ? elapsed : 0}s elapsed
                    </p>
                  </>
                )}
                <p>
                  {lastActivity?.summary ??
                    "Send a message to begin. No team or plan is required."}
                </p>
                {controls}
              </section>
              <section className="chat-overview-card">
                <h3>Working context</h3>
                <p className="mono">{detail?.workspace_path ?? project.name}</p>
                <p>
                  {detail
                    ? `${detail.provider} / ${detail.model}`
                    : "Choose a model below"}
                </p>
                <p className="faint">
                  {activeTeam
                    ? "This is the persistent solo checkout. Team builds use separate draft worktrees and never merge here automatically."
                    : "Changes stay in this checkout. Completion is not an automatic commit or verification verdict."}
                </p>
              </section>
            </div>
            {detail && (detail.mode === "team" || detail.team_builds.length > 0) && <ChatTeam detail={detail} busy={busy} command={command} events={events} now={clock} />}
            {detail && <ChatPlanning key={detail.id} chatId={detail.id} tick={tick} archived={detail.archived} frozen={activeTeam?.approved_revision != null} onChanged={onChanged} />}
            <section className="chat-overview-card">
              <h3>Turns</h3>
              {!detail?.turns.length ? (
                <p className="faint">No turns yet.</p>
              ) : (
                <ol className="chat-turn-list">
                  {detail.turns.map((turn) => (
                    <li key={turn.run.id}>
                      <span>{turn.run.prompt}</span>
                      <span className="status" data-status={turn.team ? turn.run.status : turn.node.status}>
                        {turn.team ? TEAM_PHASES[turn.team.phase] : turn.node.status}
                      </span>
                      <span className="faint">
                        run #{turn.run.id} · {turn.node.model}
                      </span>
                    </li>
                  ))}
                </ol>
              )}
            </section>
          </div>
        ) : (
          <div
            className="chat-transcript"
            ref={feed}
            onScroll={() => {
              if (feed.current)
                setFollowing(
                  feed.current.scrollHeight -
                    feed.current.scrollTop -
                    feed.current.clientHeight <
                    100,
                );
            }}
          >
            {events.length === 0 ? (
              <div className="chat-empty">
                <span className="chat-empty-mark" aria-hidden="true">
                  ⌘
                </span>
                <h2>What should we build in {project.name}?</h2>
                <p className="faint">
                  Start with a question, an idea, or a change to make.
                </p>
              </div>
            ) : (
              <div className="chat-messages" aria-label="Conversation messages">
                {events.map((event) => (
                  <Message key={event.id} event={event} />
                ))}
              </div>
            )}
            {active && detail?.live_text && (
              <article
                className="chat-message"
                aria-label="Assistant response in progress"
              >
                <BoardMarkdown source={detail.live_text} />
              </article>
            )}
            {running && (
              <div className="chat-working" role="status">
                <span className="chat-pulse" />
                {detail?.stop_requested
                  ? "Stopping the current turn…"
                  : "Working…"}
                <span className="faint">
                  {Number.isFinite(elapsed) ? elapsed : 0}s
                </span>
                {controls}
              </div>
            )}
          </div>
        )}
      </div>
      {!following && view === "chat" && (
        <button
          className="button chat-follow"
          onClick={() => setFollowing(true)}
        >
          Jump to latest
        </button>
      )}
      <form
        className="chat-composer"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <div className="chat-composer-context">
          <span>{project.name}</span>
          <span>Local</span>
          <select aria-label="Execution mode" value={mode} disabled={busy || active || detail?.archived || (id !== null && !detail) || (id === null && created.current !== null)}
            onChange={(event) => {
              const next = event.target.value === "team" ? "team" : "single";
              if (detail) void command(() => setChatMode(detail.id, next, detail.rev));
              else setNewMode(next);
            }}>
            <option value="single">Single agent · Pi</option>
            <option value="team">Team · Pi</option>
          </select>
        </div>
        <textarea
          aria-label="Message"
          placeholder={
            active
              ? "Draft your next message while the agent works…"
              : "Ask anything, or describe what to build…"
          }
          rows={3}
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing
            ) {
              event.preventDefault();
              void send();
            }
          }}
        />
        <div className="chat-composer-bottom">
          <span
            className="faint"
            title="Tools are guarded to the checkout; this is not an OS sandbox. Publishing is controlled separately."
          >
            Checkout guard
          </span>
          {id === null ? (
            <select
              aria-label="Model"
              value={modelKey}
              onChange={(event) => setModelKey(event.target.value)}
              disabled={busy || created.current !== null}
            >
              {!catalogue.length && (
                <option value="">Choose a provider in Settings</option>
              )}
              {catalogue.map((choice) => (
                <option
                  key={`${choice.provider}/${choice.model}`}
                  value={`${choice.provider}/${choice.model}`}
                >
                  {choice.model} · {choice.provider}
                </option>
              ))}
            </select>
          ) : (
            <span className="faint">{detail?.model}</span>
          )}
          <button
            type="submit"
            className="chat-send"
            aria-label="Send message"
            disabled={
              busy ||
              active ||
              detail?.archived ||
              !message.trim() ||
              (id === null ? !modelKey : detail === null)
            }
          >
            {busy ? "…" : "↑"}
          </button>
        </div>
        {mode === "team" && !active && <small className="faint">The configured project team plans first. Review and explicitly approve before building. Your solo model and history are kept.</small>}
        {active && (
          <small className="faint">
            Wait for this turn to finish, or stop it before sending a new
            instruction.
          </small>
        )}
        {id === null && (!catalogue.length || modelError) && (
          <div className="chat-model-notice">
            <span>
              {modelError ??
                "Sign in to a subscription provider to start. Local models are optional."}
            </span>
            <button type="button" className="button" onClick={onSettings}>
              Open Settings
            </button>
          </div>
        )}
      </form>
    </main>
  );
}

function Message({ event }: { event: RunEvent }) {
  return (
    <>
      {event.thinking.length > 0 && (
        <details className="chat-thinking">
          <summary>Thinking summary</summary>
          {event.thinking.map((text, index) => (
            <p key={index}>{text}</p>
          ))}
        </details>
      )}
      {event.message !== null ? (
        <article
          className={`chat-message${event.actor === "human" ? " chat-message--human" : ""}`}
          aria-label={event.actor === "human" ? "You" : "Assistant"}
        >
          <BoardMarkdown source={event.message} />
        </article>
      ) : (
        ["tool_call", "tool_result", "failed", "note", "done"].includes(
          event.kind,
        ) && (
          <div className="chat-tool" data-kind={event.kind}>
            <span aria-hidden="true">
              {event.kind === "tool_call" ? "›" : "·"}
            </span>
            {event.summary}
          </div>
        )
      )}
    </>
  );
}
