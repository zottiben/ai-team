import { useState } from "react";

import { answer, startRun, type Approval, type NodeRun, type RunDetail } from "./api";

/**
 * The prompt that starts a workflow, and the run beside it.
 *
 * The prompt is optional on purpose: with work already on the board, sending an empty one
 * means "build what is ready", which is the same rule the CLI follows. Two surfaces that
 * disagree about what an empty prompt means is worse than either behaviour.
 */
export function Prompt({
  project,
  onStarted,
}: {
  project: string | null;
  onStarted: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const start = async () => {
    if (project === null) {
      setProblem("Pick a project first - a run belongs to one.");
      return;
    }
    setBusy(true);
    setProblem(null);
    try {
      await startRun({ project, prompt: text.trim() === "" ? undefined : text.trim() });
      setText("");
      onStarted();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="prompt"
      onSubmit={(event) => {
        event.preventDefault();
        void start();
      }}
    >
      <textarea
        className="prompt__input"
        rows={3}
        value={text}
        disabled={busy}
        placeholder={
          project === null
            ? "Pick a project on the left…"
            : "What should the team build? Leave empty to build whatever the plan already has ready."
        }
        aria-label="What should the team build?"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          // Enter is a newline in a multi-line prompt; the chord submits. Getting this
          // the other way round sends half-written instructions to a team of agents.
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void start();
          }
        }}
      />
      <div className="prompt__row">
        <span className="faint">
          <span className="kbd">⌘</span>
          <span className="kbd">↵</span> to start
        </span>
        <button type="submit" className="button button--primary" disabled={busy}>
          {busy ? "Starting…" : "Start"}
        </button>
      </div>
      {problem !== null && <span className="error">{problem}</span>}
    </form>
  );
}

/**
 * The org graph: who is on the team and what each one is doing right now.
 *
 * Seats, not slices - the work graph is the board's job, and this is the question the
 * Console answers that nothing else does: is anybody actually working?
 */
export function Seats({ nodes }: { nodes: NodeRun[] }) {
  if (nodes.length === 0) {
    return <p className="faint">No seat has been dispatched yet.</p>;
  }
  return (
    <div className="seats">
      {nodes.map((node) => (
        <div key={node.id} className="seat" data-status={node.status}>
          <div className="card__row">
            <span className="status" data-status={node.status}>
              {node.role}
            </span>
            {node.attempt > 1 && <span className="faint mono">try {node.attempt}</span>}
          </div>
          <span className="faint mono">{node.slice_key ?? "—"}</span>
          <span className="faint mono">
            {node.provider}/{node.model}
          </span>
          {node.branch !== null && <span className="mono">{node.branch}</span>}
          {node.blocked_reason !== null && <span className="error">{node.blocked_reason}</span>}
        </div>
      ))}
    </div>
  );
}

/**
 * What a parked node asked, and the buttons that answer it.
 *
 * Inline rather than behind a notification: a run that is waiting is doing nothing, and
 * the cost of not noticing is the whole run sitting idle.
 */
export function Approvals({
  run,
  pending,
  onAnswered,
}: {
  run: RunDetail;
  pending: Approval[];
  onAnswered: () => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);

  if (pending.length === 0) return null;

  const respond = async (approval: Approval, choice: string) => {
    setProblem(null);
    try {
      await answer(run.id, {
        // Fall back to the run's first node: an approval recorded without one still has
        // to be answerable, and a run with a single node is the common case.
        node: approval.node_run_id ?? run.nodes[0]?.id ?? 0,
        request: approval.payload?.request_id ?? "",
        chose: choice,
      });
      onAnswered();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="approvals">
      <span className="dock__title">Waiting on you</span>
      {pending.map((approval) => (
        <div key={approval.id} className="card">
          <span>{approval.summary}</span>
          <div className="card__row">
            {(approval.payload?.options ?? [{ id: "approve", label: "Approve" }]).map((option) => (
              <button
                key={option.id}
                type="button"
                className="button button--primary"
                onClick={() => void respond(approval, option.id)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>
      ))}
      {problem !== null && <span className="error">{problem}</span>}
    </div>
  );
}
