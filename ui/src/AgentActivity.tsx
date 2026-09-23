import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  approveRunPlan,
  replyToRun,
  resumeRunNode,
  run as fetchRun,
  runEvents as fetchEvents,
  type Run,
  type RunDetail,
  type RunEvent,
} from "./api";

const ACTIVE = new Set(["queued", "planning", "running", "blocked"]);

function firstRun(runs: Run[]): number | null {
  return runs.find((run) => ACTIVE.has(run.status))?.id ?? runs[0]?.id ?? null;
}

/**
 * The human-readable side of a run.
 *
 * The event table remains the evidence, but `step / tool_call / cost` is a protocol log,
 * not a conversation. This surface turns those rows back into what the operator needs:
 * what each seat is considering, what it is doing, what it said in full, and one place to
 * answer in the same thread.
 */
export function AgentActivity({
  runs,
  workspace,
  tick,
  onOpenRun,
}: {
  runs: Run[];
  workspace: string;
  tick: number;
  onOpenRun?: (id: number) => void;
}) {
  const preferred = firstRun(runs);
  const [runId, setRunId] = useState<number | null>(preferred);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [nodeId, setNodeId] = useState<number | null>(null);
  const [message, setMessage] = useState("");
  const [sending, setSending] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const feedRef = useRef<HTMLDivElement | null>(null);
  const [following, setFollowing] = useState(true);
  const [approving, setApproving] = useState(false);
  const [approvedRun, setApprovedRun] = useState<number | null>(null);
  const [resuming, setResuming] = useState(false);

  useEffect(() => {
    if (runId === null || !runs.some((run) => run.id === runId)) setRunId(preferred);
  }, [preferred, runId, runs]);

  const load = useCallback(async () => {
    if (runId === null) {
      setDetail(null);
      setEvents([]);
      return;
    }
    try {
      const [found, log] = await Promise.all([
        fetchRun(runId, workspace),
        fetchEvents(runId, undefined, workspace),
      ]);
      setDetail(found);
      setEvents(log);
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [runId, workspace]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  useEffect(() => {
    if (detail === null || detail.nodes.length === 0) {
      setNodeId(null);
      return;
    }
    if (!detail.nodes.some((node) => node.id === nodeId)) {
      setNodeId(detail.nodes.find((node) => node.status === "running")?.id ?? detail.nodes[0]!.id);
    }
  }, [detail, nodeId]);

  const selectedNode = useMemo(
    () => detail?.nodes.find((node) => node.id === nodeId) ?? null,
    [detail, nodeId],
  );
  const conversation = useMemo(
    () => events.filter((event) => event.node_run_id === nodeId),
    [events, nodeId],
  );
  const awaitingPlanApproval = Boolean(
    detail?.status === "blocked" &&
      (detail.blocked_reason === "Plan ready for approval" ||
        detail.blocked_reason === "Preparing plan approval") &&
      detail.plan_slug &&
      approvedRun !== detail.id,
  );
  const canReply = Boolean(
    selectedNode?.status === "running" &&
      selectedNode.session_id &&
      selectedNode.replyable !== false &&
      workspace,
  );

  const scrollToLive = useCallback(() => {
    const feed = feedRef.current;
    if (feed === null) return;
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    if (typeof feed.scrollTo === "function") {
      feed.scrollTo({ top: feed.scrollHeight, behavior: reduced ? "auto" : "smooth" });
    } else {
      // jsdom and older embedded webviews do not expose scrollTo on elements.
      feed.scrollTop = feed.scrollHeight;
    }
    setFollowing(true);
  }, []);

  // Default to the live edge. If somebody scrolls up to read, stop pulling the content
  // away from them and leave one obvious way back; switching conversations resumes live
  // following because it is a new stream, not the old reading position.
  useLayoutEffect(() => {
    if (following) scrollToLive();
  }, [conversation.length, following, nodeId, scrollToLive]);

  const resume = async () => {
    if (detail === null || selectedNode === null || selectedNode.recoverable !== true) return;
    setResuming(true);
    setProblem(null);
    try {
      await resumeRunNode(detail.id, selectedNode.id, workspace);
      setOutcome(`${selectedNode.role} resumed in the same run, session, and checkout.`);
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setResuming(false);
    }
  };

  const send = async () => {
    if (detail === null || selectedNode === null || !canReply || message.trim() === "") return;
    setSending(true);
    try {
      const result = await replyToRun(detail.id, selectedNode.id, message.trim(), workspace);
      const interrupted = selectedNode.recoverable === true;
      if (interrupted) await resumeRunNode(detail.id, selectedNode.id, workspace);
      setMessage("");
      setProblem(null);
      setOutcome(
        interrupted
          ? `${selectedNode.role} resumed with your reply in the same session and checkout.`
          : result.waiting === 1
            ? `Reply queued for ${selectedNode.role}. It will continue this conversation.`
            : `${result.waiting} replies are queued for ${selectedNode.role}.`,
      );
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setSending(false);
    }
  };

  return (
    <section className="agent-activity block" aria-label="Agent activity">
      <div className="block__head agent-activity__head">
        <div>
          <h3>Agent activity</h3>
          <span className="faint">Live thinking, actions, answers, and your replies</span>
        </div>
        {runs.length > 0 && (
          <div className="agent-activity__run">
            <select
              aria-label="Activity run"
              value={runId ?? ""}
              onChange={(event) => {
                setRunId(Number(event.target.value));
                setOutcome(null);
              }}
            >
              {runs.map((run) => (
                <option key={run.id} value={run.id}>
                  Run #{run.id} · {run.status}
                </option>
              ))}
            </select>
            {onOpenRun !== undefined && runId !== null && (
              <button type="button" className="button" onClick={() => onOpenRun(runId)}>
                Evidence
              </button>
            )}
          </div>
        )}
      </div>

      {problem !== null && <p className="error">{problem}</p>}
      {detail === null ? (
        <p className="empty">Start a run to see the team think and work here.</p>
      ) : (
        <>
          <div className="agent-activity__state">
            <span className="status" data-status={detail.status}>{detail.status}</span>
            <span className="mono faint">run #{detail.id}</span>
            {detail.nodes.map((node) => (
              <button
                key={node.id}
                type="button"
                className="agent-activity__seat"
                data-selected={node.id === nodeId}
                aria-pressed={node.id === nodeId}
                onClick={() => {
                  setNodeId(node.id);
                  setFollowing(true);
                  setOutcome(null);
                }}
              >
                <span className="workspace-dot" data-status={node.status} />
                {node.role}
              </button>
            ))}
          </div>

          {awaitingPlanApproval && (
            <div className="agent-activity__approval">
              <div>
                <strong>Plan ready to review</strong>
                <span className="faint">
                  Approval continues run {detail.id} and its orchestrator conversation.
                </span>
              </div>
              <button
                type="button"
                className="button button--primary"
                disabled={approving}
                onClick={async () => {
                  setApproving(true);
                  setProblem(null);
                  try {
                    await approveRunPlan(detail.id);
                    setApprovedRun(detail.id);
                  } catch (error: unknown) {
                    setProblem(error instanceof Error ? error.message : String(error));
                  } finally {
                    setApproving(false);
                  }
                }}
              >
                {approving ? "Continuing…" : "Approve plan and build"}
              </button>
            </div>
          )}
          {approvedRun === detail.id && (
            <p className="notice">Plan approved. This same run is continuing into the build.</p>
          )}

          {selectedNode?.recoverable === true && (
            <div className="agent-activity__approval" data-status="interrupted">
              <div>
                <strong>{selectedNode.role} was interrupted</strong>
                <span className="faint">
                  Its transcript and worktree are intact. Resume the same run and Pi session.
                </span>
              </div>
              <button
                type="button"
                className="button button--primary"
                disabled={resuming}
                onClick={() => void resume()}
              >
                {resuming ? "Resuming…" : "Resume interrupted turn"}
              </button>
            </div>
          )}

          <div className="agent-activity__follow">
            {following ? (
              <span className="agent-activity__live"><span /> following live</span>
            ) : (
              <button
                type="button"
                className="button"
                aria-label="Jump to live activity"
                onClick={scrollToLive}
              >
                Jump to live
              </button>
            )}
          </div>

          <div
            ref={feedRef}
            className="agent-activity__feed"
            role="log"
            aria-label={`${selectedNode?.role ?? "agent"} conversation`}
            aria-live="polite"
            onScroll={(event) => {
              const feed = event.currentTarget;
              const atLiveEdge = feed.scrollHeight - feed.scrollTop - feed.clientHeight < 32;
              setFollowing(atLiveEdge);
            }}
          >
            <article className="activity-message activity-message--human">
              <span className="activity-message__who">You started the run</span>
              <p>{detail.prompt}</p>
            </article>
            {conversationRows(conversation, selectedNode?.status === "running")}
            {conversation.length === 0 && <p className="faint">Waiting for this agent's first update…</p>}
          </div>

          {selectedNode !== null && !canReply && (
            <p className="faint">
              {selectedNode.status === "running" && selectedNode.replyable === false
                ? "This read-only turn is evidence; it cannot be steered."
                : "Replies are available while this agent is actively working."}
            </p>
          )}
          {selectedNode !== null && canReply && (
            <form
              className="agent-activity__reply"
              onSubmit={(event) => {
                event.preventDefault();
                void send();
              }}
            >
              <label htmlFor={`reply-${detail.id}`}>
                Reply to {selectedNode.role}
                <span className="faint"> · same run, session, and checkout</span>
              </label>
              <textarea
                id={`reply-${detail.id}`}
                rows={3}
                value={message}
                placeholder="Answer its question or tell it what to do next…"
                onChange={(event) => setMessage(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault();
                    void send();
                  }
                }}
              />
              <div className="prompt__row">
                <span className="faint">This continues the thread; Talk starts unrelated seat work.</span>
                <button
                  type="submit"
                  className="button button--primary"
                  disabled={sending || message.trim() === ""}
                >
                  {sending ? "Sending…" : "Reply"}
                </button>
              </div>
              {outcome !== null && <span className="notice">{outcome}</span>}
            </form>
          )}
        </>
      )}
    </section>
  );
}

function conversationRows(events: RunEvent[], nodeRunning: boolean) {
  const seenThinking = new Set<string>();
  return events.flatMap((event, index) =>
    activityRows(event, seenThinking, nodeRunning && index === events.length - 1),
  );
}

function activityRows(event: RunEvent, seenThinking: Set<string>, latest: boolean) {
  const rows: ReactNode[] = [];
  for (const [index, thought] of event.thinking.entries()) {
    const identity = `${event.actor ?? "agent"}\u0000${thought}`;
    if (seenThinking.has(identity)) continue;
    seenThinking.add(identity);
    rows.push(
      <article key={`${event.id}-thinking-${index}`} className="activity-thinking">
        <span className="activity-message__who">{event.actor ?? "agent"} is thinking</span>
        <p>{thought}</p>
      </article>,
    );
  }

  if (event.message !== null) {
    const human = event.actor === "human";
    rows.push(
      <article
        key={`${event.id}-message`}
        className={`activity-message${human ? " activity-message--human" : ""}`}
      >
        <span className="activity-message__who">{human ? "You" : event.actor ?? "Agent"}</span>
        <p>{event.message}</p>
      </article>,
    );
    return rows;
  }

  if (["tool_call", "tool_result", "failed", "done"].includes(event.kind)) {
    rows.push(
      <div key={`${event.id}-activity`} className="activity-action" data-kind={event.kind}>
        <span className="event__kind">{event.actor ?? "team"}</span>
        <span>{event.summary}</span>
        {latest && event.kind === "tool_call" && <span className="faint">running…</span>}
      </div>,
    );
  }
  return rows;
}
