import { useState } from "react";

import { startRun, type NodeRun, type Worktree } from "./api";

/**
 * The prompt that starts a workflow, and the run beside it.
 *
 * The prompt is optional on purpose: with work already on the board, sending an empty one
 * means "build what is ready", which is the same rule the CLI follows. Two surfaces that
 * disagree about what an empty prompt means is worse than either behaviour.
 */
export function Prompt({
  project,
  workspace = null,
  onStarted,
}: {
  project: string | null;
  workspace?: Worktree | null;
  onStarted: (runId?: number) => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const start = async () => {
    if (project === null) {
      setProblem("Pick a project first - a run belongs to one.");
      return;
    }
    setBusy(true);
    setFeedback(null);
    setProblem(null);
    try {
      const receipt = await startRun({
        project,
        // Start means the full team workflow rooted in the checkout being viewed. This is
        // not just presentation context: its planner, run history, and agents must stay
        // isolated from work started in sibling checkouts.
        workspace: workspace?.path,
        prompt: text.trim() === "" ? undefined : text.trim(),
        // A new plan is reviewed before it spends maker turns. Approval resumes this
        // exact run; an empty prompt still means build already-ready work immediately.
        ...(text.trim() === "" ? {} : { approval_required: true }),
      });
      setText("");
      setFeedback(
        receipt.run_id === undefined
          ? "Request accepted. Checking the plan and preparing the run…"
          : `Run #${receipt.run_id} started. Its live evidence is open now.`,
      );
      onStarted(receipt.run_id);
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
          {workspace !== null && !workspace.main ? " · runs in this checkout" : ""}
        </span>
        <button type="submit" className="button button--primary" disabled={busy}>
          {busy ? "Starting…" : "Start"}
        </button>
      </div>
      {problem !== null && <span className="error" role="alert">{problem}</span>}
      {feedback !== null && (
        <span className="action-feedback" role="status" aria-live="polite">
          <span className="action-feedback__pulse" aria-hidden="true" />
          {feedback}
        </span>
      )}
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
