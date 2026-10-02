import { useState } from "react";
import { BoardMarkdown } from "./BoardMarkdown";
import type { ChatDetail } from "./chat-api";
import { chatPlan } from "./plan-api";
import {
  approveTeam, closeTeam, continueTeam, reviewTeam, teamCommand,
  type BuildReview, type TeamBuild,
} from "./team-api";
import "./chat-team.css";

export const TEAM_PHASES = {
  grounding: "Grounding the request", planning: "Writing the plan",
  awaiting_approval: "Awaiting your approval", building: "Building approved slices",
  blocked: "Needs attention", finished: "Execution ended",
};

export function ChatTeam({ detail, busy, command, events, now }: {
  detail: ChatDetail;
  busy: boolean;
  command: (act: () => Promise<unknown>) => Promise<void>;
  events: { node_run_id: number | null; summary: string }[];
  now: number;
}) {
  const [review, setReview] = useState<BuildReview | null>(null);
  const [closing, setClosing] = useState<{ build: TeamBuild; revision: number } | null>(null);
  const [reason, setReason] = useState("");
  const active = detail.team_builds.find((build) => build.execution.control_node_id === detail.active_node_id);
  const run = active?.execution;
  const recovery = detail.team_recovery;
  const turn = detail.turns.find((turn) => turn.team?.run_id === run?.run_id);
  const quiet = recovery?.state === "quiescent" && run?.quiescent;
  const closed = active?.closure != null;
  return (
    <section className="chat-overview-card chat-team" aria-label="Team execution">
      <h3>Team execution</h3>
      {run && active ? <>
        <strong>{TEAM_PHASES[run.phase]}</strong>
        <p className="faint">This chat · run #{run.run_id} · controller #{run.control_node_id}</p>
        {run.reason && <p className="notice">{run.reason}</p>}
        {recovery?.state === "needs_inspection" && <p className="notice">
          Process or lease ownership is uncertain. Inspect the recorded evidence; no model or cleanup will be started automatically.
        </p>}
        <div className="chat-controls">
          {run.phase === "awaiting_approval" && <button className="button button--primary" disabled={busy || detail.stop_requested}
            onClick={() => void command(async () => { setReview(await reviewTeam(run)); })}>Review build</button>}
          {(!quiet || run.approved_revision === null) && <button className="button" disabled={busy || detail.stop_requested}
            onClick={() => void command(() => teamCommand(run, "stop"))}>Stop team</button>}
          {(recovery?.state === "recoverable" || recovery?.state === "needs_inspection") && <button className="button" disabled={busy}
            onClick={() => void command(() => teamCommand(run, "recover"))}>Drain interrupted processes</button>}
          {quiet && run.approved_revision !== null && <>
            {!closed && <button className="button" disabled={busy} onClick={() => void command(() => teamCommand(run, "reconcile"))}>
              Reconcile recorded work
            </button>}
            <button className="button" disabled={busy} onClick={() => void command(async () => {
              const plan = await chatPlan(detail.id);
              setReason(active.closure?.reason ?? "");
              setClosing({ build: active, revision: plan.revision });
            })}>{closed ? "Finish closing build" : "Close and keep work…"}</button>
          </>}
        </div>
        {quiet && run.approved_revision !== null && <p className="faint">
          Reconcile checks recorded Git and lease outcomes without running a model. It may return a verified clean lease; uncertain or unfinished work stays protected.
          Continue runs only the selected retained slice within its original attempt allowance.
        </p>}
        <ul className="chat-team-members">
          {turn?.members.map(({ node, live_text }) => {
            const started = node.started_at ? Date.parse(node.started_at.endsWith("Z") ? node.started_at : `${node.started_at}Z`) : NaN;
            const live = node.status === "running" || node.status === "queued";
            return <li key={node.id}>
              <strong>{node.role}</strong> · {node.status} · {node.provider}/{node.model}
              <span className="faint"> · attempt {node.attempt}{node.slice_key ? ` · ${node.slice_key}` : ""}
                {live && Number.isFinite(started) ? ` · ${Math.max(0, Math.floor((now - started) / 1000))}s` : ""}</span>
              <p>{node.blocked_reason ?? events.filter((event) => event.node_run_id === node.id).at(-1)?.summary}</p>
              {live_text && <BoardMarkdown source={live_text} />}
            </li>;
          })}
        </ul>
        {quiet && !closed && active.slices.filter((slice) => ["retained", "leased"].includes(slice.lease_state) && slice.worktree_path && slice.branch && !slice.commit_sha && !slice.candidate_sha && !slice.release_started && slice.build_status !== "verified").map((slice) =>
          <button key={slice.slice_key} className="button" disabled={busy}
            onClick={() => void command(() => continueTeam(run, slice))}>Continue {slice.slice_key}</button>)}
      </> : <p className="faint">No team execution is active. Select Team in the composer to plan together; building always needs a separate approval.</p>}
      {review && <section className="notice" aria-label="Build approval">
        <h4>Approve exactly this review</h4>
        <p>Plan revision {review.plan.revision} · team revision {review.execution.rev}</p>
        <p className="mono">Base commit: {review.head}</p>
        <p>Only ready slices below will build in separate draft worktrees. No merge, push or publication.</p>
        <ul>{review.plan.bundle?.slices.filter((slice) => slice.status === "ready").map((slice) =>
          <li key={slice.id}><strong>{slice.key}: {slice.title}</strong><BoardMarkdown source={slice.scope_md ?? ""} /></li>)}</ul>
        <details><summary>Reviewed team</summary><ul>{review.roster.filter((agent) => agent.enabled).map((agent) =>
          <li key={agent.id}>{agent.role} · {agent.provider}/{agent.model} · {agent.read_only ? "read-only" : "maker"}</li>)}</ul></details>
        {review.dirty && <div className="error">The checkout is dirty. Commit or stash explicitly, then review again.<pre>{review.dirty}</pre></div>}
        {review.plan.bundle?.questions.some((question) => question.status === "open") && <p className="notice">Answer open questions, then review again. Answers do not approve a build.</p>}
        <button className="button button--primary" disabled={busy || !!review.dirty || !run || run.rev !== review.execution.rev || !!review.plan.bundle?.questions.some((question) => question.status === "open")}
          onClick={() => void command(async () => { await approveTeam(review); setReview(null); })}>Approve and build</button>
        <button className="button" disabled={busy} onClick={() => setReview(null)}>Dismiss review</button>
      </section>}
      {closing && <form className="notice" aria-label="Close build" onSubmit={(event) => {
        event.preventDefault();
        void command(async () => { await closeTeam(closing.build, closing.revision, reason); setClosing(null); setReason(""); });
      }}>
        <h4>Close run #{closing.build.execution.run_id} and keep all work</h4>
        <p>This permanently withdraws approval and frees this chat. Files, staged changes, sessions, draft commits and uncertain leases are kept, not merged, discarded or returned.</p>
        <p>Reviewed slices: {closing.build.slices.map((slice) => `${slice.slice_key} (revision ${slice.rev})`).join(", ")}</p>
        <label>Reason<input value={reason} readOnly={!!closing.build.closure} onChange={(event) => setReason(event.target.value)} maxLength={4000} required /></label>
        <button className="button" disabled={busy || !reason.trim()}>Confirm close and keep</button>
        <button type="button" className="button" disabled={busy} onClick={() => setClosing(null)}>Cancel</button>
      </form>}
      <h4>Drafts and retained work</h4>
      <p className="faint">Verified drafts are not merged into your solo checkout. Recorded paths are protected responsibilities, not proof of current pool possession. Uncertain work needs inspection.</p>
      {detail.team_builds.every((build) => !build.slices.length) && <p className="faint">No build has acquired work yet.</p>}
      {detail.team_builds.filter((build) => build.slices.length).map((build) => <div key={build.execution.run_id}>
        <h4>Run #{build.execution.run_id}{build.closure ? " · approval withdrawn" : ""}</h4>
        {build.closure?.issues.map((issue) => <p className="notice" key={issue}>{issue}</p>)}
        <ul>{build.slices.map((slice) => <li key={slice.slice_key}>
          <strong>{slice.slice_key}</strong> · {slice.build_status} · lease {slice.lease_state}
          <p className="mono">{slice.worktree_path ?? "No recorded checkout address"}</p>
          {slice.branch && <p className="mono">{slice.branch}</p>}
          {slice.commit_sha && <p className="mono">Verified draft: {slice.commit_sha}</p>}
          {slice.reason && <p>{slice.reason}</p>}
          {slice.release_started && slice.lease_state !== "released" && <p className="notice">Return was attempted. It will not be repeated without conclusive ownership evidence.</p>}
        </li>)}</ul>
      </div>)}
    </section>
  );
}
