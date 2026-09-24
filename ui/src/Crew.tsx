import { useCallback, useEffect, useState } from "react";

import {
  approveRunPlan,
  board as fetchBoard,
  crew as fetchCrew,
  deliverNode,
  startRun,
  workLabel,
  type Board,
  type BoardSlice,
  type Doing,
  type Member,
} from "./api";
import { pullRequestCrew, WorkGraph } from "./TeamGraph";

/** What each state means, said once, and which colour it borrows. */
const DOING: Record<Doing, { label: string; status: string; why: string }> = {
  starting: { label: "starting", status: "queued", why: "its process is coming up" },
  working: { label: "working", status: "running", why: "mid-turn" },
  parked: { label: "waiting on you", status: "parked", why: "it asked a question" },
  failed: { label: "stopped", status: "failed", why: "its last attempt failed" },
  idle: { label: "idle", status: "done", why: "finished, nothing queued" },
  untouched: { label: "not started", status: "queued", why: "never dispatched here" },
  disabled: { label: "off", status: "cancelled", why: "will not be given work" },
};

/**
 * What a seat's state is called, said once for the whole window.
 *
 * The overview shows the same six seats, and two surfaces with two vocabularies for one
 * state is two things to learn - "untouched" here and "not started" there describe the
 * same row.
 */
export function doingLabel(doing: Doing): string {
  return DOING[doing].label;
}

/** Thousands, because the interesting numbers are six figures. */
function tokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(value);
}

/**
 * The crew.
 *
 * The centre of the chair: six seats and what each is doing, without opening a run. Every
 * seat is here, including the ones doing nothing - a team where two are working is the
 * thing being looked at, and hiding the other four makes "where is the frontend" a question
 * the dashboard created.
 *
 * Ordered by the roster, never by activity: a panel whose rows swap places while you read
 * it is a panel you have to re-read.
 */
export function Crew({
  project,
  workspace,
  tick,
  onOpenRun,
  onTalk,
  showGraph = false,
  pullRequest,
}: {
  project: string;
  workspace?: string | null;
  /**
   * Set when the workspace is a pull request's worktree: that PR's key. Its crew builds it,
   * and nothing here starts a run.
   */
  pullRequest?: string;
  tick: number;
  onOpenRun: (id: number) => void;
  onTalk?: (member: Member) => void;
  showGraph?: boolean;
}) {
  const [crew, setCrew] = useState<Member[] | null>(null);
  const [board, setBoard] = useState<Board | null>(null);
  const [starting, setStarting] = useState(false);
  const [delivering, setDelivering] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const liveDelivery = board?.slices.some(
    (slice) =>
      slice.delivery?.pr_url != null &&
      slice.delivery.remote?.pr_state !== "merged" &&
      slice.delivery.remote?.pr_state !== "closed",
  ) ?? false;

  const load = useCallback(async () => {
    try {
      if (showGraph) {
        const [foundCrew, foundBoard] = await Promise.all([
          fetchCrew(project, workspace),
          fetchBoard(project, workspace),
        ]);
        setCrew(foundCrew);
        setBoard(foundBoard);
      } else {
        setCrew(await fetchCrew(project, workspace));
      }
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project, workspace, showGraph]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  useEffect(() => {
    if (!showGraph || !liveDelivery) return undefined;
    const timer = window.setInterval(() => void load(), 15_000);
    return () => window.clearInterval(timer);
  }, [liveDelivery, load, showGraph]);

  const buildReady = async (approveCurrent: boolean) => {
    setStarting(true);
    setFeedback(null);
    setProblem(null);
    try {
      const approvalRun = crew?.find(
        (member) => member.role === "orchestrator" && member.approval_run_id != null,
      )?.approval_run_id;
      const receipt = approvalRun != null
        ? await approveRunPlan(approvalRun)
        : await startRun({
            project,
            workspace,
            ...(approveCurrent ? { action: "approve_current" as const } : {}),
          });
      if (receipt.run_id !== undefined) {
        const verb = approvalRun != null || approveCurrent ? "continued" : "started";
        setFeedback(
          `Run #${receipt.run_id} ${verb}. Preparing worktrees and dispatching the team…`,
        );
        onOpenRun(receipt.run_id);
      } else {
        setFeedback("Request accepted. Checking the plan and preparing the run…");
      }
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setStarting(false);
    }
  };

  const deliver = async (
    delivery: NonNullable<BoardSlice["delivery"]>,
    action: "push" | "pr" | "merge",
  ) => {
    if (workspace === null || workspace === undefined) {
      setProblem("Select a checkout before publishing its branch.");
      return;
    }
    const key = `${delivery.node_run_id}:${action}`;
    setDelivering(key);
    setFeedback(null);
    setProblem(null);
    try {
      await deliverNode(
        delivery.run_id,
        delivery.node_run_id,
        action,
        project,
        workspace,
      );
      setFeedback(
        action === "push"
          ? `${delivery.branch} pushed. Reading the next delivery boundary…`
          : action === "pr"
            ? `${delivery.branch} pull request opened.`
            : `${delivery.branch} merge requested after required checks.`,
      );
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
      await load();
    } finally {
      setDelivering(null);
    }
  };

  if (crew === null) {
    return problem !== null
      ? <p className="error">{problem}</p>
      : <p className="empty">Seeing who is about…</p>;
  }
  if (crew.length === 0) {
    return <p className="empty">This project has no team yet.</p>;
  }

  const leaf = pullRequest !== undefined;
  const seats = leaf
    ? pullRequestCrew(
        crew,
        board?.slices.find((slice) => slice.key === pullRequest),
      )
    : crew;
  const busy = seats.filter((member) => member.doing === "working").length;
  const waiting = seats.filter((member) => member.doing === "parked").length;

  return (
    <div className="crew">
      {problem !== null && <p className="error" role="alert">{problem}</p>}
      {feedback !== null && (
        <div className="action-feedback" role="status" aria-live="polite">
          <span className="action-feedback__pulse" aria-hidden="true" />
          <span>{feedback}</span>
        </div>
      )}
      <div className="main__header">
        <h2>{showGraph ? "Work graph" : "Crew"}</h2>
        <span className="faint">
          {busy === 0 ? "nobody is working" : `${busy} working`}
          {waiting > 0 ? `, ${waiting} waiting on you` : ""}
        </span>
      </div>

      {showGraph && board !== null ? (
        <WorkGraph
          members={seats}
          board={board}
          onTalk={onTalk}
          coordinates={!leaf}
          // Building starts a run, and a run starts from the checkout above a PR's.
          onBuildReady={leaf ? undefined : buildReady}
          onDeliver={deliver}
          starting={starting}
          delivering={delivering}
        />
      ) : (
      <div className="crew__grid">
        {seats.map((member) => {
          const state = DOING[member.doing];
          return (
            <article key={member.agent_id} className="crew__card" data-doing={member.doing}>
              <div className="card__row">
                <span>{member.name}</span>
                <span className="status" data-status={state.status}>
                  {state.label}
                </span>
              </div>

              <div className="card__row">
                <span className="faint mono">
                  {member.provider}/{member.model}
                </span>
                <span className="faint">{member.read_only ? "reads" : "writes"}</span>
              </div>

              {member.slice_key !== null && (
                <div className="card__row">
                  <span className="faint">on</span>
                  <span className="mono">{workLabel(member.slice_key, member.task_key)}</span>
                  {/* An attempt above the first is worth seeing: it means this has been
                      rejected and retried, which reads very differently from progress. */}
                  {member.attempt > 1 && (
                    <span className="faint">attempt {member.attempt}</span>
                  )}
                </div>
              )}

              {/* Why it stopped beats a colour saying that it did. */}
              {member.blocked_reason !== null && (
                <span className="crew__reason">{member.blocked_reason}</span>
              )}

              {member.last_said !== null && member.blocked_reason === null && (
                <span className="faint crew__said">{member.last_said}</span>
              )}

              {member.slice_key === null && member.blocked_reason === null && (
                <span className="faint">{state.why}</span>
              )}

              <div className="card__row crew__foot">
                {member.tokens_in > 0 && (
                  <span className="faint mono">
                    {tokens(member.tokens_in)} in · {tokens(member.tokens_out)} out
                  </span>
                )}
                {member.run_id !== null && (
                  <button
                    type="button"
                    className="button"
                    onClick={() => onOpenRun(member.run_id ?? 0)}
                  >
                    Its run
                  </button>
                )}
                {onTalk !== undefined && (
                  <button type="button" className="button" onClick={() => onTalk(member)}>
                    Talk
                  </button>
                )}
              </div>
            </article>
          );
        })}
      </div>
      )}
    </div>
  );
}
