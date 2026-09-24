import { useEffect, useMemo, useState } from "react";

import {
  models as fetchModels,
  resetNodeSession,
  crewLabel,
  type Board,
  type BoardSlice,
  type Doing,
  type Member,
  type ModelChoice,
} from "./api";

const COORDINATE = new Set(["orchestrator"]);
const PLAN = new Set(["planner"]);
const CHECK = new Set(["verifier", "reviewer"]);
const DOING_LABEL: Record<Doing, string> = {
  starting: "starting",
  working: "working",
  parked: "waiting on you",
  failed: "stopped",
  idle: "idle",
  untouched: "not started",
  disabled: "off",
};

const STAGES = ["coordinate", "plan", "make", "check"] as const;
type Stage = (typeof STAGES)[number];

/**
 * The durable org graph: who exists, what each seat owns, and what context it retains.
 *
 * This intentionally does not pretend that an idle configured reviewer is current work.
 * Live tasks belong to WorkGraph; this is the slower-moving organization those tasks are
 * routed through.
 */
export function OrganizationGraph({
  members,
  workspace,
  onChanged,
}: {
  members: Member[];
  workspace: string | null;
  onChanged?: () => void | Promise<void>;
}) {
  const [catalog, setCatalog] = useState<ModelChoice[]>([]);
  const [resetting, setResetting] = useState<Set<number>>(new Set());
  const [problem, setProblem] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const liveCommand = members.some(
    (member) => member.doing === "working" && member.activity?.kind === "tool_call",
  );
  const hasActivity = members.some((member) => member.activity != null);
  const activityRevision = members.map((member) => member.activity?.at ?? "").join("|");

  useEffect(() => setNow(Date.now()), [activityRevision]);

  useEffect(() => {
    if (!hasActivity) return undefined;
    const timer = window.setInterval(
      () => setNow(Date.now()),
      liveCommand ? 1_000 : 60_000,
    );
    return () => window.clearInterval(timer);
  }, [hasActivity, liveCommand]);

  useEffect(() => {
    let current = true;
    fetchModels()
      .then((found) => {
        if (current) setCatalog(found.models ?? []);
      })
      .catch(() => {
        // A missing catalogue means an unknown denominator, never a guessed one.
      });
    return () => {
      current = false;
    };
  }, []);

  const stages = useMemo(
    () => [
      { key: "coordinate" as const, members: members.filter((m) => COORDINATE.has(m.role)) },
      { key: "plan" as const, members: members.filter((m) => PLAN.has(m.role)) },
      {
        key: "make" as const,
        members: members.filter(
          (m) => !COORDINATE.has(m.role) && !PLAN.has(m.role) && !CHECK.has(m.role),
        ),
      },
      { key: "check" as const, members: members.filter((m) => CHECK.has(m.role)) },
    ],
    [members],
  );

  const reset = async (member: Member) => {
    if (workspace === null || member.run_id === null || member.node_run_id === null) return;
    setResetting((current) => new Set(current).add(member.node_run_id!));
    setProblem(null);
    try {
      await resetNodeSession(member.run_id, member.node_run_id, workspace);
      await onChanged?.();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setResetting((current) => {
        const next = new Set(current);
        next.delete(member.node_run_id!);
        return next;
      });
    }
  };

  if (members.length === 0) return <p className="empty">This project has no team yet.</p>;

  return (
    <div className="team-graph org-graph" aria-label="Organization graph">
      {problem !== null && <p className="error">{problem}</p>}
      <div className="team-graph__flow">
        {stages.map((stage, index) => (
          <div className="team-graph__step" key={stage.key}>
            {index > 0 && <span className="team-graph__connector" aria-hidden="true" />}
            <section className="team-graph__stage" aria-label={stage.key}>
              <span className="team-graph__stage-label">{stage.key}</span>
              {stage.members.length === 0 ? (
                <span className="faint team-graph__vacant">no seat</span>
              ) : (
                stage.members.map((member) => {
                  const model = catalog.find(
                    (choice) =>
                      choice.provider === member.provider && choice.model === member.model,
                  );
                  const used = member.context_tokens ?? null;
                  const limit = model?.context_tokens ?? null;
                  const resetPending =
                    member.session_resetting_at != null ||
                    (member.node_run_id !== null && resetting.has(member.node_run_id));
                  const activity = operationalActivity(member, now);
                  const canReset =
                    member.session_active === true &&
                    member.run_id !== null &&
                    member.node_run_id !== null &&
                    workspace !== null &&
                    !active(member.doing) &&
                    !resetPending;
                  return (
                    <article
                      key={member.agent_id}
                      className="team-graph__node"
                      data-doing={member.doing}
                    >
                      <div className="team-graph__node-head">
                        <span className="workspace-dot" data-status={member.doing} />
                        <strong>{member.name}</strong>
                        <span className="status" data-status={member.doing}>
                          {DOING_LABEL[member.doing]}
                        </span>
                      </div>
                      <span className="team-graph__purpose">{purpose(member)}</span>
                      {activity !== null && (
                        <div className="team-graph__activity" data-live={activity.live}>
                          <div className="team-graph__activity-head">
                            <span>{activity.label}</span>
                            <span className="mono faint">{activity.timing}</span>
                          </div>
                          <strong title={activity.summary}>{activity.summary}</strong>
                          {activity.live && (
                            <div
                              className="team-graph__activity-meter"
                              role="progressbar"
                              aria-label={`${member.name} current command`}
                              aria-valuetext={activity.timing}
                            >
                              <span />
                            </div>
                          )}
                        </div>
                      )}
                      <div className="team-graph__facts faint">
                        <span className="mono">{member.provider}/{member.model}</span>
                        <span>{member.read_only ? "reads" : "writes"}</span>
                      </div>
                      <div className="team-graph__context">
                        <span className="faint">retained context</span>
                        <span className="mono">
                          {used === null ? "not reported" : compact(used)}
                          {limit === null ? "" : ` / ${compact(limit)}`}
                        </span>
                        <progress
                          aria-label={`${member.name} retained context`}
                          max={limit ?? 1}
                          value={used === null || limit === null ? 0 : Math.min(used, limit)}
                        />
                      </div>
                      {(member.session_active === true || member.session_resetting_at != null) && (
                        <button
                          type="button"
                          className="button team-graph__reset"
                          disabled={!canReset}
                          title={
                            active(member.doing)
                              ? "A running turn cannot be reset"
                              : member.role === "orchestrator"
                                ? "Write an ai-planner handoff, then start fresh next turn"
                                : "Start this seat fresh on its next turn"
                          }
                          onClick={() => void reset(member)}
                        >
                          {resetPending
                            ? member.role === "orchestrator"
                              ? "Writing handoff…"
                              : "Resetting…"
                            : member.role === "orchestrator"
                              ? "Handoff & reset"
                              : "New session"}
                        </button>
                      )}
                    </article>
                  );
                })
              )}
            </section>
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * The ephemeral work graph: what is moving through the organization right now.
 *
 * ai-planner remains the source of task truth. The meters below describe observable
 * handoffs: brief attached, slices shaped, slices delivered, and slices accepted. They do
 * not estimate an agent's private percentage complete from token spend.
 */
export function WorkGraph({
  members,
  board,
  onTalk,
  onBuildReady,
  onDeliver,
  starting = false,
  delivering = null,
}: {
  members: Member[];
  board: Board;
  onTalk?: (member: Member) => void;
  onBuildReady?: (approveHeld: boolean) => void | Promise<void>;
  onDeliver?: (
    delivery: NonNullable<BoardSlice["delivery"]>,
    action: "push" | "pr" | "merge",
  ) => void | Promise<void>;
  starting?: boolean;
  delivering?: string | null;
}) {
  const slices = board.slices.filter((slice) => slice.status !== "deferred");
  const planned = slices.filter((slice) => slice.status !== "draft").length;
  const checking = new Set(
    members
      .filter(
        (member) =>
          member.role === "verifier" && member.slice_key !== null && active(member.doing),
      )
      .map((member) => member.slice_key),
  );
  const delivered = slices.filter(
    (slice) => accepted(slice) || checking.has(slice.key),
  ).length;
  const verified = slices.filter((slice) => accepted(slice)).length;
  const ready = slices.filter(
    (slice) => slice.status === "ready" && slice.claimed_by === null,
  ).length;
  const approvalHeld = slices.filter((slice) => slice.approval_held === true).length;
  const makers = members.filter(
    (member) =>
      !COORDINATE.has(member.role) && !PLAN.has(member.role) && !CHECK.has(member.role),
  );
  const makerLive = makers.some((member) => active(member.doing));
  const verifierLive = members.some(
    (member) => member.role === "verifier" && active(member.doing),
  );
  const coordinatorLive = members.some(
    (member) => member.role === "orchestrator" && active(member.doing),
  );
  const plannerLive = members.some(
    (member) => member.role === "planner" && active(member.doing),
  );

  const evidence: Record<Stage, { value: number; total: number; detail: string; live: boolean }> = {
    coordinate: {
      value: board.plan === null ? 0 : 1,
      total: 1,
      detail: board.plan === null ? "awaiting a brief" : "brief handed to planning",
      live: coordinatorLive,
    },
    plan: {
      value: planned,
      total: Math.max(1, slices.length),
      detail: slices.length === 0 ? "no slices yet" : `${planned}/${slices.length} slices shaped`,
      live: plannerLive,
    },
    make: {
      value: delivered,
      total: Math.max(1, slices.length),
      detail: slices.length === 0 ? "nothing routed" : `${delivered}/${slices.length} slices delivered`,
      live: makerLive,
    },
    check: {
      value: verified,
      total: Math.max(1, slices.length),
      detail: slices.length === 0 ? "nothing to check" : `${verified}/${slices.length} verified`,
      live: verifierLive,
    },
  };

  return (
    <div className="work-graph" aria-label="Live work graph" data-live={members.some((m) => active(m.doing))}>
      <div className="work-graph__stages">
        {STAGES.map((stage, index) => {
          const metric = evidence[stage];
          const carrying = index > 0 && edgeLive(stage, evidence);
          return (
            <div className="work-graph__stage-wrap" key={stage}>
              {index > 0 && (
                <span className="work-graph__edge" data-live={carrying} aria-hidden="true" />
              )}
              <section className="work-graph__stage" data-live={metric.live} aria-label={`${stage} work`}>
                <span className="team-graph__stage-label">{stage}</span>
                <strong className="work-graph__reading mono">
                  {metric.value}/{metric.total}
                </strong>
                <span className="faint">{metric.detail}</span>
                <progress
                  aria-label={`${stage} evidence`}
                  max={metric.total}
                  value={metric.value}
                />
              </section>
            </div>
          );
        })}
      </div>

      {board.plan === null ? (
        <p className="empty">No work graph yet. Direct the orchestrator to create one.</p>
      ) : slices.length === 0 ? (
        <p className="empty">The plan exists, but it has no active slices.</p>
      ) : (
        <div className="work-graph__tasks" aria-label="Plan slices">
          {slices.map((slice) => {
            return (
              <article className="work-task" data-live={slice.status === "active"} key={slice.key}>
                <div className="work-task__head">
                  <span className="mono">{slice.key}</span>
                  <strong>{slice.title}</strong>
                  <span
                    className="status"
                    data-status={slice.approval_held ? "queued" : slice.status}
                  >
                    {slice.approval_held ? "awaiting approval" : slice.status.replace("_", " ")}
                  </span>
                </div>
                <div className="work-task__route" aria-label={`${slice.key} route`}>
                  {(["plan", "make", "check"] as const).map((stage, index) => (
                    <span
                      key={stage}
                      className="work-task__segment"
                      data-state={routeState(slice, index, members)}
                    >
                      {stage}
                    </span>
                  ))}
                </div>
                <div className="work-task__foot faint">
                  <span>
                    {crewLabel(slice) === null ? "unrouted" : `built by ${crewLabel(slice)}`}
                  </span>
                  {slice.blocked_reason !== null && (
                    <span className={slice.approval_held ? "faint" : "error"}>
                      {slice.blocked_reason}
                    </span>
                  )}
                </div>
                {slice.delivery !== null && slice.delivery !== undefined && accepted(slice) && (
                  <DeliveryControls
                    sliceKey={slice.key}
                    delivery={slice.delivery}
                    onDeliver={onDeliver}
                    delivering={delivering}
                  />
                )}
              </article>
            );
          })}
        </div>
      )}

      {(onTalk !== undefined || (onBuildReady !== undefined && (approvalHeld > 0 || ready > 0))) && (
        <div className="work-graph__controls" aria-label="Direct the team">
          {onBuildReady !== undefined && approvalHeld > 0 && (
            <button
              type="button"
              className="button button--primary"
              disabled={starting}
              onClick={() => void onBuildReady(true)}
            >
              {starting ? "Continuing…" : "Approve plan & build"}
            </button>
          )}
          {onBuildReady !== undefined && approvalHeld === 0 && ready > 0 && (
            <button
              type="button"
              className="button button--primary"
              disabled={starting}
              onClick={() => void onBuildReady(false)}
            >
              {starting ? "Starting…" : `Build ${ready} ready ${ready === 1 ? "slice" : "slices"}`}
            </button>
          )}
          {onTalk !== undefined && members
            .filter((member) => member.doing !== "disabled")
            .map((member) => (
              <button
                type="button"
                className="button"
                key={member.agent_id}
                onClick={() => onTalk(member)}
              >
                {`Talk to ${member.name}`}
              </button>
            ))}
        </div>
      )}
    </div>
  );
}

function DeliveryControls({
  sliceKey,
  delivery,
  onDeliver,
  delivering,
}: {
  sliceKey: string;
  delivery: NonNullable<BoardSlice["delivery"]>;
  onDeliver?: (
    delivery: NonNullable<BoardSlice["delivery"]>,
    action: "push" | "pr" | "merge",
  ) => void | Promise<void>;
  delivering: string | null;
}) {
  const policy = delivery.policy;
  const action = delivery.pushed_at == null ? "push" : delivery.pr_url == null ? "pr" : "merge";
  const merged = delivery.remote?.pr_state === "merged";
  const complete = merged || delivery.merge_requested_at != null;
  const claimedAt = Date.parse(delivery.delivery_claimed_at ?? "");
  const liveClaim =
    delivery.delivery_claim === action &&
    Number.isFinite(claimedAt) &&
    Date.now() - claimedAt < 5 * 60 * 1000;
  const busy = liveClaim || delivering === `${delivery.node_run_id}:${action}`;
  const label = action === "push" ? "Push branch" : action === "pr" ? "Open PR" : "Merge after checks";
  const manual =
    action === "push"
      ? `git push -u origin ${delivery.branch}`
      : action === "pr"
        ? `gh pr create --head ${delivery.branch}`
        : `gh pr merge ${delivery.pr_url ?? "<url>"} --auto --merge`;

  return (
    <div className="work-task__delivery" aria-label={`${sliceKey} delivery`}>
      <code className="mono">{delivery.branch}</code>
      <span className="status" data-status="done">committed</span>
      <span className="status" data-status={delivery.pushed_at == null ? "queued" : "done"}>
        {delivery.pushed_at == null ? "not pushed" : "pushed"}
      </span>
      {delivery.pr_url !== null ? (
        <a href={delivery.pr_url} target="_blank" rel="noreferrer">
          {delivery.remote === null ? "pull request" : `PR ${delivery.remote.pr_state}`}
        </a>
      ) : (
        <span className="faint">no PR</span>
      )}
      {delivery.remote !== null && (
        <span
          className="status"
          data-status={
            delivery.remote.checks === "failed"
              ? "failed"
              : delivery.remote.checks === "pending"
                ? "running"
                : delivery.remote.checks === "passed" || delivery.remote.checks === "none"
                  ? "done"
                  : "queued"
          }
        >
          {delivery.remote.checks === "none"
            ? "no checks"
            : `checks ${delivery.remote.checks}`}
        </span>
      )}
      {complete ? (
        <span className="status" data-status="done">{merged ? "merged" : "merge requested"}</span>
      ) : policy[action] === "ask" && onDeliver !== undefined ? (
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={() => void onDeliver(delivery, action)}
        >
          {busy ? "Working…" : label}
        </button>
      ) : policy[action] === "auto" ? (
        <span className="faint">{busy ? `${label}…` : `${label} automatically`}</span>
      ) : (
        <code className="mono">{manual}</code>
      )}
      {delivery.delivery_error !== null && (
        <span className="error" role="alert">{delivery.delivery_error}</span>
      )}
    </div>
  );
}

/** Kept as the public name used by older callers; it now means the stable organization. */
export function TeamGraph(props: Parameters<typeof OrganizationGraph>[0]) {
  return <OrganizationGraph {...props} />;
}

function purpose(member: Member): string {
  switch (member.role) {
    case "orchestrator":
      return "coordinates the work graph";
    case "planner":
      return "shapes ai-planner slices";
    case "verifier":
      return "runs gates and an independent verdict";
    case "reviewer":
      return "available for landed-change review";
    case "backend":
      return "builds server and data changes";
    case "frontend":
      return "builds product interfaces";
    default:
      return "builds product changes";
  }
}

function operationalActivity(
  member: Member,
  now: number,
): { label: string; summary: string; timing: string; live: boolean } | null {
  const activity = member.activity;
  if (activity === null || activity === undefined) return null;
  const activeNow = active(member.doing);
  const live = activeNow && activity.kind === "tool_call";
  return {
    label: live ? "Current command" : activeNow ? "Current activity" : "Latest update",
    summary: activity.summary,
    timing: live ? `running for ${elapsed(activity.at, now)}` : relative(activity.at, now),
    live,
  };
}

function elapsed(at: string, now: number): string {
  const then = Date.parse(at);
  const seconds = Number.isFinite(then) ? Math.max(0, Math.floor((now - then) / 1_000)) : 0;
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

function relative(at: string, now: number): string {
  const value = elapsed(at, now);
  return value === "0s" ? "just now" : `${value} ago`;
}

function edgeLive(stage: Stage, evidence: Record<Stage, { live: boolean }>): boolean {
  if (stage === "plan") return evidence.coordinate.live || evidence.plan.live;
  if (stage === "make") return evidence.plan.live || evidence.make.live;
  if (stage === "check") return evidence.make.live || evidence.check.live;
  return false;
}

function routeState(
  slice: BoardSlice,
  index: number,
  members: Member[],
): "done" | "live" | "waiting" {
  if (accepted(slice)) return "done";
  const verifierLive = members.some(
    (member) =>
      member.role === "verifier" && member.slice_key === slice.key && active(member.doing),
  );
  if (verifierLive) return index < 2 ? "done" : "live";
  if (slice.status === "active") return index === 0 ? "done" : index === 1 ? "live" : "waiting";
  if (slice.status === "ready" || slice.status === "blocked") {
    return index === 0 ? "done" : "waiting";
  }
  const plannerLive = members.some(
    (member) => member.role === "planner" && active(member.doing),
  );
  return index === 0 && plannerLive ? "live" : "waiting";
}

function accepted(slice: BoardSlice): boolean {
  return slice.status === "in_review" || slice.status === "done";
}

function active(doing: Member["doing"]): boolean {
  return doing === "starting" || doing === "working";
}

function compact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return String(value);
}
