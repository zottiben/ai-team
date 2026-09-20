import { useState } from "react";

import { sayTo, type Member } from "./api";

/**
 * Saying something to one seat.
 *
 * The two acts this can perform are genuinely different, and which one is about to happen
 * is said before anybody presses send. A seat mid-turn takes the message into what it is
 * already doing - an interruption. An idle seat has no process, so reaching it means
 * starting a turn in a leased worktree: minutes of work rather than a remark.
 *
 * A single box that silently did either would be a surprise waiting to happen, which is the
 * whole reason this says which.
 */
export function Talk({ member, onClose, onSent }: {
  member: Member;
  onClose: () => void;
  onSent: () => void;
}) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Mirrors the server's own reading of the same rows. It can be a moment stale - the
  // process may have gone since - so the *attempt* reports the truth and this only sets
  // expectations.
  const act =
    member.doing === "disabled" ? "nothing" : member.reachable ? "interrupt" : "start";

  const send = async () => {
    if (message.trim() === "") return;
    setBusy(true);
    try {
      const reached = await sayTo(member.agent_id, message);
      setProblem(null);
      setOutcome(
        reached.reached === "interrupted"
          ? `${member.name} has it, mid-turn.`
          : reached.reached === "started"
            ? `Starting a turn for ${member.name}. It will appear in the runs below.`
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
        <span className="dock__title">Talk to {member.name}</span>
        <button type="button" className="button" onClick={onClose}>
          Close
        </button>
      </div>

      <div className="card__row">
        <span className="faint mono">
          {member.provider}/{member.model}
        </span>
        {member.slice_key !== null && <span className="faint">on {member.slice_key}</span>}
      </div>

      {/* Said before, not after. These are different acts. */}
      <p className={act === "interrupt" ? "notice" : "faint"}>
        {act === "interrupt"
          ? "It is mid-turn, so this lands in the middle of what it is doing."
          : act === "start"
            ? "It is idle, so this starts a turn for it in its own worktree. That takes a few minutes."
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
          aria-label={`message for ${member.role}`}
          placeholder={
            act === "interrupt"
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
          {busy ? "Sending…" : act === "interrupt" ? "Interrupt" : "Start a turn"}
        </button>
      </form>
    </aside>
  );
}
