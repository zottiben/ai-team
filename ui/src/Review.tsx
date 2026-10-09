import { useCallback, useEffect, useRef, useState } from "react";
import { FileView } from "./ReviewFile";

import {
  addComment,
  resolveComment,
  review as fetchReview,
  reviews as fetchReviews,
  submitReview,
  type Review as ReviewSummary,
  type ReviewDetail,
  type Submitted,
} from "./api";

/**
 * Reviewing an agent's diff.
 *
 * A comment is anchored to a file, a side and a line, because a comment on a removed line
 * and one on an added line at the same number are different comments. The numbers come
 * from the server's parse of git's own output rather than from counting rows here - a
 * comment that lands one line off reads as a confident remark about different code.
 */
export function Review({
  project = null,
  workspace = null,
  tick,
  initial = null,
  onOpenedInitial,
}: {
  project?: string | null;
  workspace?: string | null;
  tick: number;
  /** A review to open on arrival - how Today hands one over. */
  initial?: number | null;
  onOpenedInitial?: () => void;
}) {
  const [list, setList] = useState<ReviewSummary[]>([]);
  const [open, setOpen] = useState<number | null>(initial);

  // Handed over once: a later tick must not drag the view back to it.
  const handed = useRef<number | null>(null);
  useEffect(() => {
    if (initial !== null && handed.current !== initial) {
      handed.current = initial;
      setOpen(initial);
      onOpenedInitial?.();
    }
  }, [initial, onOpenedInitial]);
  const [detail, setDetail] = useState<ReviewDetail | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Submitted | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await fetchReviews(project, workspace, true));
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project, workspace]);

  const loadDetail = useCallback(async () => {
    if (open === null) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await fetchReview(open, workspace));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [open, workspace]);

  useEffect(() => {
    void load();
  }, [load, tick]);
  useEffect(() => {
    void loadDetail();
  }, [loadDetail, tick]);

  const submit = async (status: string) => {
    if (open === null) return;
    try {
      setOutcome(await submitReview(open, status, workspace));
      await Promise.all([load(), loadDetail()]);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  if (open !== null && detail !== null) {
    return (
      <div className="review">
        <div className="main__header review__head">
          <button type="button" className="button" onClick={() => setOpen(null)}>
            Back
          </button>
          <h2>{detail.title}</h2>
          {detail.branch !== null && <span className="faint mono">{detail.branch}</span>}
        </div>

        {problem !== null && <p className="error">{problem}</p>}
        {outcome !== null && <Outcome outcome={outcome} />}

        {/* Said before the human decides what to write, because the two are different
            kinds of feedback: one lands in a worktree that still exists, the other is a
            note for whoever picks the work up next. */}
        <p className="faint">
          {detail.steerable
            ? "The agent that wrote this is still working - submitting sends your comments straight to it."
            : detail.follows_up
              ? "That agent has finished, but its pull request is still open where it was built - submitting sends your comments back there, and the whole pull request is checked again."
              : "That agent has finished - submitting puts your comments on the plan as a new slice."}
        </p>

        {detail.files.map((file) => (
          <FileView
            key={`${detail.id}:${file.path}`}
            file={file}
            reviewKey={`review:${detail.project_id}:${detail.id}`}
            comments={detail.comments}
            onComment={async (body, anchor) => {
              await addComment(detail.id, { body, ...anchor });
              await loadDetail();
            }}
            onResolve={async (id) => {
              await resolveComment(id);
              await loadDetail();
            }}
          />
        ))}

        {detail.files.length === 0 && <p className="empty">This branch changed nothing.</p>}

        <div className="review__actions">
          <button type="button" className="button" onClick={() => void submit("approved")}>
            Approve
          </button>
          <button
            type="button"
            className="button button--primary"
            onClick={() => void submit("changes_requested")}
          >
            Request changes
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="review">
      <div className="main__header">
        <h2>Review</h2>
        <span className="faint">{list.length} open</span>
      </div>
      {problem !== null && <p className="error">{problem}</p>}
      {list.length === 0 && <p className="empty">Nothing to review.</p>}
      <div className="list">
        {list.map((entry) => (
          <button
            type="button"
            key={entry.id}
            className="card review__entry"
            onClick={() => {
              setOutcome(null);
              setOpen(entry.id);
            }}
          >
            <div className="card__row">
              <span>{entry.title}</span>
              <span className="faint mono">{entry.branch ?? ""}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

function Outcome({ outcome }: { outcome: Submitted }) {
  // Which of the two things happened is the whole point of the submit, so it is said
  // plainly rather than left to be inferred from the list refreshing.
  if (outcome.outcome === "steered") {
    return (
      <p className="notice">
        Sent {outcome.comments} comment(s) to the agent - it is working on them now.{" "}
        {/* The plan is amended either way; this says whether the orchestrator also heard it
            while mid-turn, which is the difference between now and its next plan read. */}
        {outcome.told_orchestrator
          ? "The orchestrator has it too."
          : "The plan is updated, so the orchestrator will see it next time it looks."}
      </p>
    );
  }
  if (outcome.outcome === "followed_up") {
    return (
      <p className="notice">
        That agent had finished, so run #{outcome.run_id} took {outcome.comments} comment(s) back
        into <span className="mono">{outcome.slice_key}</span>'s worktree - on its branch, in
        the same conversation - and will check the whole pull request again.
      </p>
    );
  }
  if (outcome.outcome === "planned") {
    return (
      <p className="notice">
        That agent had finished, so {outcome.comments} comment(s) became slice{" "}
        <span className="mono">{outcome.slice_key}</span>.
      </p>
    );
  }
  return <p className="notice">Approved.</p>;
}
