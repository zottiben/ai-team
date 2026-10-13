import {
  Fragment,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { BoardMarkdown } from "./BoardMarkdown";
import { CHAT_PANELS, ChatContext, type ChatPanel } from "./ChatContext";
import { TEAM_PHASES } from "./ChatTeam";
import { ChatWorkspace } from "./ChatWorkspace";
import { ChatOutcome, chatOutcome } from "./ChatOutcome";
import { CancelledInstructions, FollowupReceipt, QueuedInstructions, followupEvent, useFollowupArrivals } from "./ChatFollowups";
import logo from "../../crates/ai-team-desktop/icons/mark.svg";
import { setChatMode } from "./team-api";
import { models, type ModelChoice, type Project, type Reasoning, type RunEvent } from "./api";
import { SelectChevron } from "./SelectChevron";
import {
  archiveChat,
  restoreChat,
  chat,
  chatEvents,
  createChat,
  renameChat,
  resumeChat,
  sendChat,
  stopChat,
  queueChatFollowup,
  setChatThinking,
  type ChatDetail,
  type ChatFollowup,
  type ImageInput,
} from "./chat-api";

const ImageDrafts = lazy(() => import("./ChatImages").then(m => ({ default: m.ImageDrafts })));
const SentImages = lazy(() => import("./ChatImages").then(m => ({ default: m.SentImages })));

const Editor = lazy(() => import("./Editor").then((module) => ({ default: module.Editor })));
const Terminal = lazy(() => import("./Terminal").then((module) => ({ default: module.TerminalPane })));
// Lazy so the capability lookup and the seven labels stay out of the eager bundle.
const ThinkingSelect = lazy(() => import("./ThinkingSelect").then(m => ({ default: m.ThinkingSelect })));

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
  initialPanel = null,
}: {
  id: number | null;
  project: Project;
  tick: number;
  onCreated: (id: number) => void;
  onChanged: () => void;
  onArchived: () => void;
  onSettings: () => void;
  initialPanel?: ChatPanel | null;
}) {
  const [detail, setDetail] = useState<ChatDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [panel, setPanel] = useState<ChatPanel | null>(initialPanel);
  const [openedPanels, setOpenedPanels] = useState<ReadonlySet<ChatPanel>>(new Set(initialPanel ? [initialPanel] : []));
  const [tool, setTool] = useState<"editor" | "terminal" | null>(null);
  const [openedTools, setOpenedTools] = useState<Record<string, { editor: boolean; terminal: boolean }>>({});
  const [newWorkspace, setNewWorkspace] = useState<string | null>(null);
  const [workspacePending, setWorkspacePending] = useState(false);
  const [catalogue, setCatalogue] = useState<ModelChoice[]>([]);
  const [modelKey, setModelKey] = useState("");
  const modelPicked = useRef(false);
  const [newMode, setNewMode] = useState<"single" | "team">("single");
  const [newReasoning, setNewReasoning] = useState<ChatDetail["reasoning"]>("high");
  const [modelError, setModelError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [images, setImages] = useState<ImageInput[]>([]);
  const [readingImages, setReadingImages] = useState(false);
  const reading = useRef(false);
  const imagePicker = useRef<HTMLInputElement | null>(null);
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
  const submission = useRef<{ body: string; images: ImageInput[]; key: string; workspaceEpoch: number; reasoning?: ChatDetail["reasoning"] } | null>(null);
  const queuedSubmission = useRef<{ body: string; images: ImageInput[]; key: string; node: number; kind: ChatFollowup["kind"] } | null>(null);
  const feed = useRef<HTMLDivElement | null>(null);
  const composer = useRef<HTMLTextAreaElement | null>(null);
  const openPanel = (next: ChatPanel) => {
    setPanel(next);
    setTool(null);
    setOpenedPanels(previous => new Set([...previous, next]));
  };
  const closePanel = () => {
    if (panel) document.getElementById(`chat-context-${panel}`)?.focus();
    setPanel(null);
  };

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
        setDetail(previous => previous && previous.rev > found.rev ? previous : found);
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
        if (!modelPicked.current && found.models[0]) {
          modelPicked.current = true;
          if (!found.models[0].thinking) setNewReasoning("none");
        }
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
  const outcome = chatOutcome(detail, events);
  const activeTeam = detail?.turns.find((turn) => turn.team?.control_node_id === detail.active_node_id)?.team;
  const mode = detail?.mode ?? newMode;
  const queued = detail?.followups?.find((item) => item.state === "queued");
  const arriving = useFollowupArrivals(detail?.followups);
  const messageEvents = [...events, ...(detail?.followups ?? []).filter(item => item.node_id !== null && (item.state === "starting" || item.state === "delivered") && !events.some(event => event.actor === "human" && event.node_run_id === item.node_id)).map(followupEvent)];
  const activeTurn = detail?.turns.find(turn => turn.node.id === detail.active_node_id);
  const thinkingValue: Reasoning | "" = active ? activeTurn?.node.reasoning ?? "" : detail?.reasoning ?? newReasoning;
  // Which model's limits apply. An existing chat's are its own; a new one's come from the
  // picker, split on the first `/` because a model id may contain more of them.
  const thinkingModel = detail
    ? { provider: detail.provider, model: detail.model }
    : modelKey
      ? { provider: modelKey.slice(0, modelKey.indexOf("/")), model: modelKey.slice(modelKey.indexOf("/") + 1) }
      : null;
  const canQueue = detail?.state === "running" && !activeTeam && mode === "single" && !detail.stop_requested && !queued && !workspacePending;
  const soloActive = active && !activeTeam && mode === "single";
  const hasDraft = !!message.trim() || images.length > 0;
  const showStop = soloActive && (!hasDraft || !running);
  const stopping = !!detail?.stop_requested && detail.state !== "interrupted";
  const actionLabel = showStop ? stopping ? "Stopping…" : "Stop turn" : soloActive ? "Queue follow-up" : "Send message";
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
  }, [following, events.length, detail?.live_text, detail?.followups, tool]);

  const attach = async (files: File[]) => {
    if (!files.length || busy || reading.current || detail?.archived) return;
    reading.current = true;
    setReadingImages(true);
    setProblem(null);
    try {
      const { readImages } = await import("./ChatImages");
      const added = await readImages(files, images.length);
      if (alive.current) setImages(previous => [...previous, ...added]);
    } catch (error: unknown) {
      if (alive.current) setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      reading.current = false;
      if (alive.current) setReadingImages(false);
    }
  };

  const send = async () => {
    if (busy || readingImages || active || queued || workspacePending || !hasDraft || (id !== null && detail === null))
      return;
    const chosen = catalogue.find(
      (choice) => `${choice.provider}/${choice.model}` === modelKey,
    );
    if (id === null && !chosen) return;
    setBusy(true);
    setProblem(null);
    const body = message.trim() || "Please inspect the attached images.";
    if (submission.current?.body !== body || submission.current.images !== images)
      submission.current = { body, images, key: crypto.randomUUID(), workspaceEpoch: detail?.workspace_epoch ?? 0, reasoning: mode === "single" ? detail?.reasoning ?? newReasoning : undefined };
    try {
      let target = id ?? created.current;
      if (target === null && chosen) {
        const draft = await createChat({
          project: project.slug,
          ...(newWorkspace ? { workspace: newWorkspace } : {}),
          provider: chosen.provider,
          model: chosen.model,
          reasoning: newReasoning,
          ...(newMode === "team" ? { mode: newMode } : {}),
        });
        target = draft.id;
        created.current = target;
      }
      if (target === null) return;
      await sendChat(target, body, submission.current.key, submission.current.workspaceEpoch, images.length ? images : undefined, submission.current.reasoning);
      if (!alive.current) return;
      setMessage("");
      setImages([]);
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

  const queue = async (kind: ChatFollowup["kind"]) => {
    if (busy || readingImages || !canQueue || detail?.active_node_id == null || !hasDraft) return;
    const body = message.trim() || "Please inspect the attached images.";
    if (queuedSubmission.current?.body !== body || queuedSubmission.current.kind !== kind || queuedSubmission.current.images !== images)
      queuedSubmission.current = { body, images, key: crypto.randomUUID(), node: detail.active_node_id, kind };
    const request = queuedSubmission.current;
    setBusy(true);
    setProblem(null);
    try {
      if (images.length) await queueChatFollowup(detail.id, request.node, request.body, request.key, request.kind, images);
      else await queueChatFollowup(detail.id, request.node, request.body, request.key, request.kind);
      if (!alive.current) return;
      setMessage("");
      setImages([]);
      queuedSubmission.current = null;
      onChanged();
      await load();
    } catch (error: unknown) {
      if (alive.current) setProblem(error instanceof Error ? error.message : String(error));
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
    !soloActive || !detail?.can_resume ? null : (
      <div className="chat-controls">
        <button
          className="button button--primary"
          disabled={busy}
          onClick={() =>
            void command(() => resumeChat(detail.id, detail.active_node_id!))
          }
        >
          {detail.stop_requested && queued?.kind === "steer" ? "Finish stop and send steering" : "Resume turn"}
        </button>
      </div>
    );

  return (
    <main
      className="chat-workspace"
      aria-label={`${project.name} conversation`}
      onDragOver={event => { if (event.dataTransfer.types.includes("Files")) event.preventDefault(); }}
      onDrop={event => { if (event.dataTransfer.types.includes("Files")) { event.preventDefault(); void attach(Array.from(event.dataTransfer.files)); } }}
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
            {(["editor", "terminal"] as const).map((name) => <button key={name} className="button" aria-pressed={tool === name} aria-controls={`chat-tool-${name}-${encodeURIComponent(detail.workspace_path)}`} onClick={() => {
              setTool(tool === name ? null : name);
              setOpenedTools((opened) => ({ ...opened, [detail.workspace_path]: { editor: false, terminal: false, ...opened[detail.workspace_path], [name]: true } }));
            }}>{name === "editor" ? "Editor" : "Terminal"}</button>)}
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
              disabled={busy || active || !!queued}
              onClick={() =>
                void command(async () => {
                  if (detail.archived) await restoreChat(detail.id);
                  else {
                    await archiveChat(detail.id);
                    if (alive.current) onArchived();
                  }
                })
              }
            >
              {detail.archived ? "Restore chat" : "Archive"}
            </button>
          </div>
        )}
      </header>
      <div className="chat-toolbar" role="group" aria-label="Chat tools">
        {(Object.keys(CHAT_PANELS) as ChatPanel[]).map(name => <button
          key={name}
          id={`chat-context-${name}`}
          aria-expanded={panel === name && tool === null}
          aria-controls="chat-context"
          disabled={!detail}
          onClick={() => panel === name && tool === null ? closePanel() : openPanel(name)}
        >{CHAT_PANELS[name]}</button>)}
        {controls}
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
          {" "}No work was resumed. Restart AI Team to retry automatic recovery.
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
            : detail.stop_requested && queued?.kind === "steer" ? " Finish the pending stop before sending the queued steering. Stop turn instead cancels that instruction." : " Resume it, or stop it and send a new instruction."}
        </div>
      )}
      {activeTeam && <div className="chat-notice notice">
        <strong>{TEAM_PHASES[activeTeam.phase]}.</strong> {activeTeam.reason}
        <button className="button" onClick={() => openPanel("work")}>Open team controls</button>
      </div>}
      {latest?.node.blocked_reason && !active && (
        <p className="chat-notice notice">{latest.node.blocked_reason}</p>
      )}
      <div className="chat-body" data-context={panel !== null && tool === null}>
      <div className="chat-conversation">
      <div className="chat-panel" hidden={tool !== null}>
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
            {messageEvents.length === 0 ? (
              <div className="chat-empty">
                <img className="chat-empty-mark" src={logo} alt="" width="64" height="64" />
                <h2>What should we build in {project.name}?</h2>
                <p className="faint">
                  Start with a question, an idea, or a change to make.
                </p>
              </div>
            ) : (
              <div className="chat-messages" aria-label="Conversation messages">
                {messageEvents.map(event => {
                  const turn = detail?.turns.find(turn => turn.node.id === event.node_run_id);
                  const followup = event.actor === "human" ? detail?.followups?.find(item => item.node_id === event.node_run_id && (item.state === "starting" || item.state === "delivered")) : undefined;
                  return <Fragment key={followup ? `followup:${followup.id}` : event.id}>
                    <Message event={event} result={event.id === outcome?.response} followup={followup} status={turn?.node.status} arriving={arriving} />
                    {detail && turn && event.actor === "human" && !!turn.images?.length && <Suspense fallback={<p>Loading images…</p>}><SentImages chat={detail.id} node={turn.node.id} images={turn.images}/></Suspense>}
                  </Fragment>;
                })}
              </div>
            )}
            <CancelledInstructions items={detail?.followups}/>
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
              </div>
            )}
          </div>
      </div>
      {detail && Object.entries(openedTools).map(([workspace, opened]) => <section key={workspace} className="chat-tools-panel" hidden={tool === null || workspace !== detail.workspace_path} aria-label="Chat checkout tools">
        <div className="card__row"><strong className="mono">{workspace}</strong><button className="button" onClick={() => setTool(null)}>Close tools</button></div>
        <p className="faint">These are operator tools, not covered by the agent's checkout guard. This is the persistent chat checkout, not a team draft. Save edits before leaving this chat.</p>
        <div id={`chat-tool-editor-${encodeURIComponent(workspace)}`} className="chat-tool-pane" hidden={tool !== "editor"}>
          {opened.editor && <Suspense fallback={<p>Loading editor…</p>}><Editor project={project.slug} workspace={workspace} node={null} visible={tool === "editor" && workspace === detail.workspace_path} /></Suspense>}
        </div>
        <div id={`chat-tool-terminal-${encodeURIComponent(workspace)}`} className="chat-tool-pane" hidden={tool !== "terminal"}>
          {opened.terminal && <Suspense fallback={<p>Loading terminal…</p>}><Terminal project={project.slug} workspace={workspace} node={null} /></Suspense>}
        </div>
      </section>)}
      {!following && tool === null && (
        <button
          className="button chat-follow"
          onClick={() => setFollowing(true)}
        >
          Jump to latest
        </button>
      )}
      {tool === null && outcome && <ChatOutcome outcome={outcome} onAction={action => {
        if (action === "reply") composer.current?.focus();
        else openPanel(action === "review" ? "review" : "work");
      }} />}
      <QueuedInstructions detail={detail} active={active} busy={busy} command={command}/>
      <form
        className="chat-composer"
        onSubmit={(event) => {
          event.preventDefault();
          if (canQueue) void queue("follow_up");
          else void send();
        }}
      >
        <div className="chat-composer-context">
          <span title={project.name}>{project.name}</span>
          <ChatWorkspace project={project.slug} detail={detail} selected={newWorkspace} tick={tick} disabled={busy || !!detail?.archived || (id !== null && !detail) || (id === null && created.current !== null)} onSelect={setNewWorkspace} onChanged={() => { setTool(null); onChanged(); void load(); }} onPending={setWorkspacePending} />
          <span className="chat-select">
          <select aria-label="Execution mode" value={mode} disabled={busy || active || !!queued || detail?.archived || (id !== null && !detail) || (id === null && created.current !== null)}
            onChange={(event) => {
              const next = event.target.value === "team" ? "team" : "single";
              if (detail) void command(() => setChatMode(detail.id, next, detail.rev));
              else setNewMode(next);
            }}>
            <option value="single">Single agent · Pi</option>
            <option value="team">Team · Pi</option>
          </select>
          <SelectChevron />
          </span>
        </div>
        {!!images.length && <Suspense fallback={<p>Loading image previews…</p>}><ImageDrafts images={images} disabled={busy || readingImages} onRemove={index => setImages(previous => previous.filter((_, i) => i !== index))}/></Suspense>}
        {readingImages && <p role="status">Reading images…</p>}
        <textarea
          ref={composer}
          onPaste={event => { const files = Array.from(event.clipboardData.files); if (files.length) { event.preventDefault(); void attach(files); } }}
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
              if (canQueue) void queue("follow_up");
              else void send();
            }
          }}
        />
        <div className="chat-composer-bottom">
          <input ref={imagePicker} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden aria-label="Choose images" onChange={event => { const files = Array.from(event.target.files ?? []); event.target.value = ""; void attach(files); }}/>
          <button type="button" className="button" disabled={busy || readingImages || detail?.archived} onClick={() => imagePicker.current?.click()} title="Drop or paste images, or choose files. Up to 4 images, 5 MiB each.">Attach image</button>
          <span
            className="faint"
            title="Tools are guarded to the checkout; this is not an OS sandbox. Publishing is controlled separately."
          >
            Checkout guard
          </span>
          {id === null ? (
            <span className="chat-select chat-model-select">
            <select
              aria-label="Model"
              value={modelKey}
              onChange={(event) => {
                modelPicked.current = true;
                setModelKey(event.target.value);
                if (catalogue.find(choice => `${choice.provider}/${choice.model}` === event.target.value)?.thinking === false) setNewReasoning("none");
              }}
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
            <SelectChevron />
            </span>
          ) : (
            <span className="chat-model-name faint" title={detail?.model}>{detail?.model}</span>
          )}
          {mode === "single" ? <Suspense fallback={<span className="faint">Thinking…</span>}>
            <ThinkingSelect
              provider={thinkingModel?.provider ?? null}
              model={thinkingModel?.model ?? null}
              value={thinkingValue}
              title="Requested thinking passed to Pi. Provider/model support can constrain it. Change while idle for the next turn; running and queued work keep their settings."
              disabled={busy || active || !!queued || !!detail?.archived || !!submission.current || (id !== null && !detail) || (id === null && (created.current !== null || !catalogue.find(choice => `${choice.provider}/${choice.model}` === modelKey)?.thinking))}
              onChange={value => {
                if (!detail) setNewReasoning(value);
                else void command(async () => {
                  const changed = await setChatThinking(detail.id,value,detail.rev);
                  if (alive.current) setDetail(previous => previous && previous.rev <= changed.rev ? { ...previous, ...changed } : previous);
                });
              }}
            />
          </Suspense> : <span className="faint">Team thinking · per-seat settings</span>}
          {soloActive && running && hasDraft && <button type="button" className="button" disabled={busy || !canQueue} onClick={() => void queue("steer")}>Stop and steer</button>}
          <button
            type={showStop ? "button" : "submit"}
            className="chat-send"
            data-working={soloActive && running}
            aria-label={actionLabel}
            title={actionLabel}
            onClick={() => {
              if (showStop && detail?.active_node_id != null)
                void command(() => stopChat(detail.id, detail.active_node_id!));
            }}
            disabled={showStop ? busy || stopping || detail?.orphan_running :
              busy || readingImages ||
              (active && !canQueue) ||
              !!queued ||
              workspacePending ||
              detail?.archived ||
              !hasDraft ||
              (id === null ? !modelKey : detail === null)
            }
          >
            {busy ? "…" : showStop
              ? <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true"><rect x="7" y="7" width="10" height="10" rx="1" fill="currentColor" /></svg>
              : <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12h14m-6-6 6 6-6 6" /></svg>}
          </button>
        </div>
        {(queuedSubmission.current || submission.current) && problem && <small className="faint">
          {queuedSubmission.current ? `This draft still targets turn #${queuedSubmission.current.node}.` : "This message still targets the checkout selected for its original send."} Clearing it does not cancel work already accepted by the server; inspect this chat's history and receipts first.
          <button className="button" type="button" disabled={busy} onClick={() => { queuedSubmission.current = null; submission.current = null; setMessage(""); setImages([]); setProblem(null); }}>Clear this draft</button>
        </small>}
        {soloActive && activeTurn?.node.reasoning !== detail?.reasoning && <small className="faint">Queued follow-ups use this chat's {detail?.reasoning === "none" ? "off" : detail?.reasoning} thinking preference.</small>}
        {mode === "team" && !active && <small className="faint">The configured project team plans first. Review and explicitly approve before building. Your solo model and history are kept.</small>}
        {active && (
          <small className="faint">
            {activeTeam || mode === "team" ? "Use Work's exact team controls. A message cannot bypass build approval." : "Queue one follow-up after successful completion, or Stop and steer to drain this turn and start a new one. No instruction is injected into a running tool. Stop turn cancels the queued instruction."}
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
      </div>
      {detail && <ChatContext detail={detail} panel={panel} opened={openedPanels} hidden={tool !== null}
        status={LABELS[detail.state]} tick={tick} busy={busy} command={command} events={events} elapsed={elapsed} now={clock}
        onChanged={onChanged} onClose={closePanel} onFeedback={text => {
          setMessage(old => old ? `${old}\n\n${text}` : text);
          composer.current?.focus();
        }} />}
      </div>
    </main>
  );
}

function Message({ event, result, followup, status, arriving }: { event: RunEvent; result: boolean; followup?: ChatFollowup; status?: string; arriving: ReadonlySet<number> }) {
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
          className={`chat-message${event.actor === "human" ? " chat-message--human" : ""}${result ? " chat-message--result" : ""}${followup && arriving.has(followup.id) ? " chat-message--arriving" : ""}`}
          aria-label={event.actor === "human" ? "You" : "Assistant"}
        >
          <BoardMarkdown source={event.message} />
          {followup && <FollowupReceipt item={followup} status={status}/>}
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
