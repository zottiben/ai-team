import { useCallback, useEffect, useState } from "react";

import {
  addComment,
  resolveComment,
  review as fetchReview,
  reviews as fetchReviews,
  submitReview,
  type Comment,
  type DiffLine,
  type FileDiff,
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
export function Review({ tick }: { tick: number }) {
  const [list, setList] = useState<ReviewSummary[]>([]);
  const [open, setOpen] = useState<number | null>(null);
  const [detail, setDetail] = useState<ReviewDetail | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Submitted | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await fetchReviews(true));
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  const loadDetail = useCallback(async () => {
    if (open === null) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await fetchReview(open));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [open]);

  useEffect(() => {
    void load();
  }, [load, tick]);
  useEffect(() => {
    void loadDetail();
  }, [loadDetail, tick]);

  const submit = async (status: string) => {
    if (open === null) return;
    try {
      setOutcome(await submitReview(open, status));
      await Promise.all([load(), loadDetail()]);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  if (open !== null && detail !== null) {
    return (
      <div className="review">
        <div className="main__header">
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
            : "That agent has finished - submitting puts your comments on the plan as a new slice."}
        </p>

        {detail.files.map((file) => (
          <FileView
            key={file.path}
            file={file}
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
            className="card"
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

function FileView({
  file,
  comments,
  onComment,
  onResolve,
}: {
  file: FileDiff;
  comments: Comment[];
  onComment: (
    body: string,
    anchor: { file_path: string; side: "old" | "new"; line_start: number; line_end: number },
  ) => Promise<void>;
  onResolve: (id: number) => Promise<void>;
}) {
  const [writing, setWriting] = useState<string | null>(null);
  const [body, setBody] = useState("");

  const mine = comments.filter((comment) => comment.file_path === file.path);

  return (
    <section className="diff">
      <div className="diff__head">
        <span className="mono">{file.path}</span>
        {file.old_path !== null && <span className="faint mono">was {file.old_path}</span>}
        <span className="status" data-status={file.status === "removed" ? "failed" : "done"}>
          {file.status}
        </span>
        <span className="faint mono">
          +{file.additions} -{file.deletions}
        </span>
      </div>

      {/* git decides what is binary, and rendering its bytes as text is how a review
          turns into a screenful of noise. */}
      {file.binary && <p className="faint">Binary file - not shown.</p>}

      {file.hunks.map((hunk) => (
        <div key={hunk.header} className="diff__hunk">
          <div className="diff__hunk-head mono">{hunk.header}</div>
          {hunk.lines.map((line) => {
            const side = line.kind === "removed" ? "old" : "new";
            const number = line.kind === "removed" ? line.old : line.new;
            const key = `${side}:${number}`;
            const on = mine.filter(
              (comment) => comment.side === side && comment.line_start === number,
            );
            return (
              <div key={key}>
                <LineRow line={line} onAdd={() => setWriting(writing === key ? null : key)} />
                {on.map((comment) => (
                  <Thread key={comment.id} comment={comment} onResolve={onResolve} />
                ))}
                {writing === key && number !== null && (
                  <form
                    className="diff__compose"
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (body.trim() === "") return;
                      void onComment(body, {
                        file_path: file.path,
                        side,
                        line_start: number,
                        line_end: number,
                      }).then(() => {
                        setBody("");
                        setWriting(null);
                      });
                    }}
                  >
                    <textarea
                      aria-label={`comment on ${file.path} ${side} line ${number}`}
                      value={body}
                      onChange={(event) => setBody(event.target.value)}
                      rows={2}
                    />
                    <button type="submit" className="button button--primary">
                      Comment
                    </button>
                  </form>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </section>
  );
}

function LineRow({ line, onAdd }: { line: DiffLine; onAdd: () => void }) {
  return (
    <div className="diff__line" data-kind={line.kind}>
      <span className="diff__num mono">{line.old ?? ""}</span>
      <span className="diff__num mono">{line.new ?? ""}</span>
      <button
        type="button"
        className="diff__add"
        aria-label={`comment on ${line.kind === "removed" ? "old" : "new"} line ${
          line.kind === "removed" ? line.old : line.new
        }`}
        onClick={onAdd}
      >
        +
      </button>
      <code className="diff__text">{line.text === "" ? " " : line.text}</code>
    </div>
  );
}

function Thread({
  comment,
  onResolve,
}: {
  comment: Comment;
  onResolve: (id: number) => Promise<void>;
}) {
  return (
    <div className="diff__comment" data-status={comment.status}>
      <div className="card__row">
        <span className="faint">{comment.author}</span>
        {comment.status === "open" ? (
          <button type="button" className="button" onClick={() => void onResolve(comment.id)}>
            Resolve
          </button>
        ) : (
          <span className="faint">{comment.status}</span>
        )}
      </div>
      <span>{comment.body}</span>
    </div>
  );
}
