import { useCallback, useEffect, useState } from "react";

import { crew as fetchCrew, type Doing, type Member } from "./api";

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
  tick,
  onOpenRun,
  onTalk,
}: {
  project: string;
  tick: number;
  onOpenRun: (id: number) => void;
  onTalk?: (member: Member) => void;
}) {
  const [crew, setCrew] = useState<Member[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setCrew(await fetchCrew(project));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  if (problem !== null) return <p className="error">{problem}</p>;
  if (crew === null) return <p className="empty">Seeing who is about…</p>;
  if (crew.length === 0) {
    return <p className="empty">This project has no team yet.</p>;
  }

  const busy = crew.filter((member) => member.doing === "working").length;
  const waiting = crew.filter((member) => member.doing === "parked").length;

  return (
    <div className="crew">
      <div className="main__header">
        <h2>Crew</h2>
        <span className="faint">
          {busy === 0 ? "nobody is working" : `${busy} working`}
          {waiting > 0 ? `, ${waiting} waiting on you` : ""}
        </span>
      </div>

      <div className="crew__grid">
        {crew.map((member) => {
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
                  <span className="mono">{member.slice_key}</span>
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
    </div>
  );
}
