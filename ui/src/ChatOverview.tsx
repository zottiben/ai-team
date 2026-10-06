import { useEffect, useState } from "react";

import { RepoGraph, zoneColour } from "./Map";
import type { RunEvent } from "./api";
import type { ChatDetail } from "./chat-api";
import {
  chatOverview,
  chatOverviewMap,
  type ChatMapView,
  type ChatOverview as Live,
  type ChatSeat,
} from "./chat-overview-api";
import { chatActivity, compact, elapsedSince, isWorking, operational, percent } from "./chat-overview";
import { chatPlan, type ChatPlan, type PlanStatus } from "./plan-api";
import "./chat-overview.css";

/** The columns a plan tally reads as, in the order work moves through them. */
const STAGES: PlanStatus[] = ["draft", "ready", "active", "in_review", "blocked", "done"];

/** How many touched paths one seat lists before the rest go behind a disclosure. */
const SHOWN = 6;

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * This chat's Overview: what it is doing, the repository it is doing it in, and which
 * agent is on which file.
 *
 * Every figure is read from one chat id. The project's latest run cannot appear here,
 * because there is no request on this page that takes a project - which is the point of
 * the surface, not an implementation detail of it.
 *
 * The split between what is refetched and what is not matters. The map is a filesystem
 * walk and a repository does not change shape because a token arrived, so it is read once
 * per chat; the seats, their commands and their files are SQL, and are re-read on every
 * tick - which is what makes the picture move.
 */
export function ChatOverview({
  detail,
  status,
  tick,
  events,
  elapsed,
  now,
}: {
  detail: ChatDetail;
  status: string;
  tick: number;
  events: RunEvent[];
  elapsed: number;
  now: number;
}) {
  const [live, setLive] = useState<Live | null>(null);
  const [view, setView] = useState<ChatMapView | null>(null);
  const [plan, setPlan] = useState<ChatPlan | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [mapProblem, setMapProblem] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setMapProblem(null);
    setView(null);
    chatOverviewMap(detail.id)
      .then((found) => {
        if (current) setView(found);
      })
      .catch((error: unknown) => {
        if (current) setMapProblem(message(error));
      });
    return () => {
      current = false;
    };
  }, [detail.id, detail.workspace_path, detail.mode]);

  useEffect(() => {
    let current = true;
    Promise.all([chatOverview(detail.id), chatPlan(detail.id)])
      .then(([found, planned]) => {
        if (!current) return;
        setLive(found);
        setPlan(planned);
        setProblem(null);
      })
      .catch((error: unknown) => {
        if (current) setProblem(message(error));
      });
    return () => {
      current = false;
    };
  }, [detail.id, tick]);

  const running = detail.state === "running" || detail.state === "stopping";
  const active = events.filter(
    (event) => event.node_run_id === (detail.active_node_id ?? detail.turns.at(-1)?.node.id),
  );
  const seats = live?.seats ?? [];
  const activity = view === null ? null : chatActivity(view.map, seats);
  const busy = (activity?.working.size ?? 0) > 0;
  const lit = activity === null ? 0 : activity.hot.size + activity.warm.size;
  const slices = plan?.bundle?.slices ?? [];
  const owned = view === null ? 0 : view.map.files - view.map.unowned;

  return (
    <>
      <section className="chat-overview-card">
        <h3>Execution</h3>
        <strong>{status}</strong>
        <p>
          {detail.mode === "team"
            ? "Pi team · chat-owned draft worktrees"
            : "Single Pi agent · local checkout"}
        </p>
        {running && (
          <>
            <div className="chat-live-meter" role="progressbar" aria-label="Agent working" />
            <p className="mono">{Number.isFinite(elapsed) ? elapsed : 0}s elapsed</p>
          </>
        )}
        <p>{active.at(-1)?.summary ?? "Send a message to begin. No team or plan is required."}</p>
        <p className="faint">
          {detail.turns.length} {detail.turns.length === 1 ? "turn" : "turns"} in this conversation
        </p>
      </section>

      {(problem !== null || mapProblem !== null) && (
        <p className="error" role="alert">
          {problem ?? mapProblem}
        </p>
      )}

      <div className="figures chat-figures">
        <div className="figure">
          <span className="figure__label faint">files</span>
          <span className="figure__value mono">{view === null ? "…" : compact(view.map.files)}</span>
        </div>
        <div className="figure">
          <span className="figure__label faint">covered</span>
          <span className="figure__value mono">
            {view === null || view.map.files === 0 ? "--" : percent(owned / view.map.files)}
          </span>
        </div>
        <div className="figure">
          <span className="figure__label faint">turns</span>
          <span className="figure__value mono">{live?.totals.turns ?? "…"}</span>
        </div>
        <div className="figure">
          <span className="figure__label faint">tokens</span>
          <span className="figure__value mono">
            {live === null
              ? "…"
              : compact(live.totals.usage.tokens_in + live.totals.usage.tokens_out)}
          </span>
        </div>
        <div className="figure">
          <span className="figure__label faint">files edited</span>
          <span className="figure__value mono">
            {live === null ? "…" : `${live.totals.files_written}/${live.totals.files_touched}`}
          </span>
        </div>
      </div>

      <section className="chat-overview-card" aria-label="Repository map">
        <h3>Repository</h3>
        <p className="faint">
          {busy
            ? `${lit} ${lit === 1 ? "path" : "paths"} lit · ${activity?.working.size} working`
            : "every path, and the seat whose zone claims it in this chat"}
        </p>
        {view === null ? (
          <p className="faint">Walking this chat's checkout…</p>
        ) : (
          <>
            <ul className="legend">
              {view.map.zones
                .filter((zone) => zone.owns > 0)
                .sort((a, b) => b.owns - a.owns)
                .map((zone) => (
                  <li key={zone.role} data-working={activity?.working.has(zone.role) === true}>
                    <span
                      className="legend__swatch"
                      style={{ background: zoneColour(zone.role, roles(view)) }}
                    />
                    {zone.role}
                    <span className="faint">{zone.owns}</span>
                  </li>
                ))}
              {view.map.unowned > 0 && (
                <li>
                  <span
                    className="legend__swatch"
                    style={{ background: "var(--zone-none-default)" }}
                  />
                  unclaimed
                  <span className="faint">{view.map.unowned}</span>
                </li>
              )}
            </ul>
            {activity !== null && <RepoGraph map={view.map} activity={activity} />}
            <p className="faint">
              {view.map.unowned === 0
                ? "Every path is inside a seat's zone, so nothing here would be reported unroutable."
                : `${view.map.unowned} ${view.map.unowned === 1 ? "path belongs" : "paths belong"} to no seat of this chat. Work touching one is reported undone rather than given to somebody.`}
              {view.map.truncated && " The walk stopped early, so this is a sample of the checkout."}
            </p>
          </>
        )}
      </section>

      <section className="chat-overview-card" aria-label="Agents and files">
        <h3>Agents</h3>
        {live === null ? (
          <p className="faint">Reading this chat's seats…</p>
        ) : seats.length === 0 ? (
          <p className="faint">
            No agent has worked in this chat yet. Send a message and its seat appears here.
          </p>
        ) : (
          <>
            <div className="team-graph chat-agent-graph" role="group" aria-label="Agent execution graph">
              <div className="team-graph__flow">
                {(["coordinate", "plan", "make", "check"] as const).map(stage => ({
                  stage, seats: seats.filter(seat => (seat.role === "orchestrator" ? "coordinate" : seat.role === "planner" ? "plan" : ["reviewer", "verifier"].includes(seat.role) ? "check" : "make") === stage),
                })).filter(group => group.seats.length > 0).map((group, index) => <div className="team-graph__step" key={group.stage}>
                  {index > 0 && <span className="team-graph__connector" aria-hidden="true" data-live={group.seats.some(isWorking)} />}
                  <section className="team-graph__stage" aria-label={group.stage}>
                    <span className="team-graph__stage-label">{group.stage}</span>
                    {group.seats.map(seat => <Seat key={seat.role} seat={seat} now={now} workspace={live.workspace} />)}
                  </section>
                </div>)}
              </div>
            </div>
            <p className="faint">Role order, not task dependencies. Only this chat's recorded seats and activity are shown, with at most 60 paths per seat.</p>
            {live.nodes_read < live.nodes_total && (
              <p className="faint">
                Files and totals are read from the newest {live.nodes_read} of{" "}
                {live.nodes_total} agent turns in this chat.
              </p>
            )}
          </>
        )}
      </section>

      {detail.turns.length > 0 && <section className="chat-overview-card" aria-label="Recent turns">
        <h3>Recent turns</h3>
        <ol className="chat-turn-list">{detail.turns.slice(-5).reverse().map(turn => <li key={turn.run.id}>
          <span>{turn.run.prompt}</span>
          <span className="status" data-status={turn.team ? turn.run.status : turn.node.status}>{turn.team ? turn.run.status : turn.node.status}</span>
          <span className="faint">run #{turn.run.id} · {turn.node.model}</span>
        </li>)}</ol>
        <p className="faint">Full turn history and exact execution controls are in Work.</p>
      </section>}

      <section className="chat-overview-card" aria-label="Plan">
        <h3>Plan</h3>
        {plan?.bundle == null ? (
          <p className="faint">
            No plan in this chat yet. Ask for one in the conversation, or open Board to write it.
          </p>
        ) : (
          <>
            <strong>{plan.bundle.plan.title}</strong>
            <ol className="path">
              {STAGES.map((stage) => {
                const counted = slices.filter((slice) => slice.status === stage).length;
                return (
                  <li
                    key={stage}
                    className="path__stage"
                    data-empty={counted === 0}
                    data-active={stage === "active" && counted > 0}
                  >
                    <span className="faint">{stage.replace("_", " ")}</span>
                    <span className="mono path__count">{counted}</span>
                  </li>
                );
              })}
            </ol>
            {/* One mark per slice, in plan order - the shape of the work, not a chart. */}
            <div className="tape">
              {slices.map((slice) => (
                <span
                  key={slice.key}
                  className="tape__tick"
                  data-status={slice.status}
                  data-live={seats.some(
                    (seat) => isWorking(seat) && seat.slice_key === slice.key,
                  )}
                  title={`${slice.key} · ${slice.status}`}
                />
              ))}
            </div>
            <p className="faint">
              {plan.bundle.questions.filter((question) => question.status === "open").length} open{" "}
              {plan.bundle.questions.filter((question) => question.status === "open").length === 1
                ? "question"
                : "questions"}{" "}
              · {slices.length} {slices.length === 1 ? "slice" : "slices"}
            </p>
          </>
        )}
      </section>

      {detail.team_builds.length > 0 && (
        <section className="chat-overview-card" aria-label="Drafts">
          <h3>Drafts</h3>
          <p className="faint">
            A verified draft is a commit this chat owns. Nothing here is merged or published
            without its own approval.
          </p>
          <ul className="chat-overview-drafts">
            {detail.team_builds.flatMap((build) =>
              build.slices.map((slice) => (
                <li key={`${build.execution.run_id}:${slice.slice_key}`}>
                  <span className="mono">{slice.slice_key}</span>
                  <span className="status" data-status={slice.build_status}>
                    {slice.build_status}
                  </span>
                  <span className="faint">
                    lease {slice.lease_state}
                    {slice.branch === null ? "" : ` · ${slice.branch}`}
                  </span>
                </li>
              )),
            )}
          </ul>
        </section>
      )}

      <section className="chat-overview-card">
        <h3>Working context</h3>
        <p className="mono">{detail.workspace_path}</p>
        <p>
          {detail.provider} / {detail.model}
        </p>
        <p className="faint">
          {detail.mode === "team"
            ? "This is the persistent solo checkout. Team builds use separate draft worktrees and never merge here automatically."
            : "Changes stay in this checkout. Completion is not an automatic commit or verification verdict."}
        </p>
      </section>
    </>
  );
}

/** The zone roles, sorted - the same order [`zoneColour`] keys its palette on. */
function roles(view: ChatMapView): string[] {
  return [...new Set(view.map.zones.map((zone) => zone.role))].sort();
}

/**
 * One seat: who it is, what it is doing now, and the files it has actually touched.
 *
 * Organization and live work stay separate on purpose. The durable half is the model it
 * runs on and the lease it works in; the live half is one command, how long it has been
 * running, and what it last said. There is no percentage here, because nothing in the
 * evidence knows how far through a task an agent is.
 */
function Seat({ seat, now, workspace }: { seat: ChatSeat; now: number; workspace: string }) {
  const [all, setAll] = useState(false);
  const doing = operational(seat, now);
  const interrupted = seat.live && seat.supervised === false;
  const shown = all ? seat.touches : seat.touches.slice(0, SHOWN);
  return (
    <article className="chat-seat" data-working={isWorking(seat)} aria-label={`${seat.role} seat`}>
      <div className="chat-seat__head">
        <span className="workspace-dot" data-status={seat.status} />
        <strong>{seat.role}</strong>
        <span className="status" data-status={interrupted ? "failed" : seat.status}>
          {interrupted ? "no live process" : seat.status}
        </span>
        {seat.slice_key !== null && <span className="mono faint">{seat.slice_key}</span>}
        {seat.attempt > 1 && <span className="faint">try {seat.attempt}</span>}
      </div>

      {doing !== null && (
        <div className="chat-seat__activity" data-live={doing.live}>
          <div className="chat-seat__activity-head">
            <span>{doing.label}</span>
            <span className="mono faint">{doing.timing}</span>
          </div>
          <strong title={doing.detail ?? doing.summary}>
            {doing.summary}
            {doing.detail === null ? "" : ` · ${doing.detail}`}
          </strong>
          {doing.live && (
            <div
              className="chat-seat__meter"
              role="progressbar"
              aria-label={`${seat.role} current command`}
              aria-valuetext={doing.timing}
            >
              <span />
            </div>
          )}
        </div>
      )}
      {seat.said !== null && <p className="chat-seat__said">{seat.said}</p>}
      {seat.blocked_reason !== null && <p className="notice">{seat.blocked_reason}</p>}

      <div className="chat-seat__facts faint">
        <span className="mono">
          {seat.provider}/{seat.model}
        </span>
        <span>
          {seat.runs} {seat.runs === 1 ? "turn" : "turns"} · {seat.steps} steps
        </span>
        <span className="mono">
          {compact(seat.usage.tokens_in + seat.usage.tokens_out)} tokens
        </span>
        {seat.context_tokens !== null && (
          <span className="mono">{compact(seat.context_tokens)} retained</span>
        )}
        {seat.started_at !== null && (
          <span>
            {isWorking(seat) ? "started" : "last ran"} {elapsedSince(seat.started_at, now)} ago
          </span>
        )}
      </div>
      {seat.worktree !== null && seat.worktree !== workspace && (
        <p className="mono faint">{seat.worktree}</p>
      )}

      {seat.touches.length === 0 ? (
        <p className="faint">No file touched in the turns read.</p>
      ) : (
        <ul className="chat-seat__files">
          {shown.map((touch) => (
            <li key={touch.path} data-wrote={touch.writes > 0}>
              <span className="mono">{touch.path}</span>
              <span className="faint">
                {touch.writes > 0
                  ? `${touch.writes} ${touch.writes === 1 ? "edit" : "edits"}`
                  : `${touch.reads} ${touch.reads === 1 ? "read" : "reads"}`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {seat.touches.length > SHOWN && (
        <button type="button" className="button" onClick={() => setAll((open) => !open)}>
          {all ? "Show fewer files" : `Show all ${seat.touches.length} files`}
        </button>
      )}
      {seat.outside > 0 && (
        <p className="faint">
          {seat.outside} {seat.outside === 1 ? "path" : "paths"} it named resolve outside this
          checkout and are not drawn on the map.
        </p>
      )}
    </article>
  );
}
