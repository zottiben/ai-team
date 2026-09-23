import { useState } from "react";

import { sayTo, workLabel, type Member } from "./api";

/**
 * Saying something to one seat.
 *
 * The two acts this can perform are genuinely different, and which one is about to happen
 * is said before anybody presses send. A seat mid-turn cannot be reached inside the turn -
 * a Pi turn is a process reading one prompt - so the message waits and is the first thing
 * that seat is given next. An idle seat has no turn to wait for, so reaching it means
 * starting one in a leased worktree: minutes of work rather than a remark.
 *
 * A single box that silently did either would be a surprise waiting to happen, which is the
 * whole reason this says which.
 */
export function Talk({ member, workspace, onClose, onSent }: {
  member: Member;
  workspace?: string | null;
  onClose: () => void;
  onSent: () => void;
}) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Mirrors the server's own reading of the same rows. It can be a moment stale - the
  // process may have gone since - so the *attempt* reports the truth and this only sets
  // expectations. The orchestrator is deliberately not a generic read-only seat: this is
  // the control plane for the whole graph.
  const orchestrator = member.role === "orchestrator";
  const act = member.doing === "disabled"
    ? "nothing"
    : member.reachable
      ? "queue"
      : orchestrator && member.approval_run_id != null
        ? "approve"
        : orchestrator
          ? "coordinate"
          : "start";

  const send = async () => {
    if (message.trim() === "") return;
    setBusy(true);
    try {
      const reached = await sayTo(member.agent_id, message, workspace);
      setProblem(null);
      setOutcome(
        reached.reached === "queued"
          ? reached.waiting === 1
            ? `${member.name} is working. It gets this next.`
            : `${member.name} is working. It gets this and ${reached.waiting - 1} other message(s) next.`
          : reached.reached === "started"
            ? `Starting a direct turn for ${member.name}. It will appear in the runs below.`
            : reached.reached === "coordinating"
              ? "The orchestrator is grounding this request. The planner will turn its brief into a plan for your approval."
              : reached.reached === "continued"
                ? `Run #${reached.run_id} is continuing into maker dispatch and independent checks.`
                : reached.because,
      );
      setMessage("");
      onSent();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside className="dock">
      <div className="dock__header">
        <span className="dock__title">
          {orchestrator ? "Direct the orchestrator" : `Talk to ${member.name}`}
        </span>
        <button type="button" className="button" onClick={onClose}>
          Close
        </button>
      </div>

      <div className="card__row">
        <span className="faint mono">
          {member.provider}/{member.model}
        </span>
        {member.slice_key !== null && (
          <span className="faint">on {workLabel(member.slice_key, member.task_key)}</span>
        )}
      </div>

      {/* Said before, not after. These are different acts. */}
      <p className={act === "queue" || act === "approve" ? "notice" : "faint"}>
        {act === "queue"
          ? orchestrator
            ? "The orchestrator is coordinating now. This direction waits for its next turn in the same run."
            : "It is mid-turn. This waits, and is the first thing it is given next."
          : act === "approve"
            ? `Run #${member.approval_run_id} is waiting for plan approval. Your direction is kept in the orchestrator conversation, then the same run dispatches its makers.`
            : act === "coordinate"
              ? "This starts the team workflow in this checkout: coordinate → plan → approve → make → check. The orchestrator delegates; it never tries to edit code itself."
              : act === "start"
                ? workspace
                  ? "It is idle, so this starts a direct turn for it in the selected worktree. That takes a few minutes."
                  : "It is idle, so this starts a direct turn for it in its own worktree. That takes a few minutes."
                : "It is switched off, so it would never be given this."}
      </p>

      {problem !== null && <p className="error">{problem}</p>}
      {outcome !== null && <p className="notice">{outcome}</p>}

      <form
        className="talk__form"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <textarea
          aria-label={orchestrator ? "direction for orchestrator" : `message for ${member.role}`}
          placeholder={
            act === "approve"
              ? "The plan looks good. Start building."
              : act === "coordinate"
                ? "Describe the outcome you want the team to deliver."
                : act === "queue"
                  ? "Stop using the old helper - use the new one."
                  : "Have a look at the failing test in src/lib.rs and fix it."
          }
          rows={5}
          value={message}
          onChange={(event) => setMessage(event.target.value)}
          // The chord submits and Enter is a newline, for the same reason as the prompt:
          // the other way round sends half-written instructions to an agent.
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void send();
            }
          }}
        />
        <button
          type="submit"
          className="button button--primary"
          disabled={busy || message.trim() === "" || act === "nothing"}
        >
          {busy
            ? act === "approve"
              ? "Continuing…"
              : "Sending…"
            : act === "queue"
              ? orchestrator ? "Send direction" : "Send"
              : act === "approve"
                ? "Approve plan & build"
                : act === "coordinate"
                  ? "Start team run"
                  : "Start a turn"}
        </button>
      </form>
    </aside>
  );
}
