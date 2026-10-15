import { useMemo, useState } from "react";
import { useFileProgress } from "./review-progress";
import type { Comment, DiffLine, FileDiff } from "./api";

/** A line longer than this is not for reading: a minified bundle, or generated output. */
const UNREADABLE_LINE = 1000;
type CommentAnchor = { file_path: string; side: "old" | "new"; line_start: number; line_end: number };
type CommentSender = (body: string, anchor: CommentAnchor) => Promise<void | boolean>;
type Writing = { key: string; source: string; scope: string; lineText: string; anchor: CommentAnchor; submit: CommentSender; replaySafe: boolean };

/** The longest line of a file's diff. A reduce, since a spread of a large diff's lines
 * would pass more arguments than a call may take. */
function longestLine(file: FileDiff): number {
  return file.hunks.reduce(
    (longest, hunk) =>
      hunk.lines.reduce((inHunk, line) => Math.max(inHunk, line.text.length), longest),
    0,
  );
}

export function FileView({
  file,
  reviewKey,
  revision,
  commentScope,
  retryCapturedRequest = false,
  reason,
  comments = [],
  onComment,
  onResolve,
}: {
  file: FileDiff;
  reviewKey?: string;
  revision?: string;
  commentScope?: string;
  /** Only for senders backed by the exact-request idempotency controller. */
  retryCapturedRequest?: boolean;
  reason?: string | null;
  comments?: Comment[];
  onComment?: (
    body: string,
    anchor: { file_path: string; side: "old" | "new"; line_start: number; line_end: number },
  ) => Promise<void | boolean>;
  onResolve?: (id: number) => Promise<void>;
}) {
  const [writing, setWriting] = useState<Writing | null>(null);
  const [body, setBody] = useState("");
  const [attemptedBody, setAttemptedBody] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const mine = comments.filter((comment) => comment.file_path === file.path);
  // A minified file starts collapsed: a committed bundle is one line tens of thousands of
  // characters long, and shown whole it buried the rest of the PR. Open from the start
  // when it has comments, so no thread is hidden.
  const longest = useMemo(() => longestLine(file), [file]);
  const progress = useFileProgress(file, reviewKey, longest > UNREADABLE_LINE && mine.length === 0, reason, revision);
  const scope = commentScope ?? progress.source;
  const anchorChanged = writing !== null && (writing.source !== progress.source || writing.scope !== scope);
  const collapsed = progress.collapsed && !anchorChanged, { viewed } = progress;
  // A lost response is retried through the captured sender with the same body. The
  // owning request controller, not a fresh diff, retains its idempotency identity.
  const retry = !!writing?.replaySafe && attemptedBody !== null && body === attemptedBody;
  const canSubmit = !saving && !!body.trim() && (retry || (!anchorChanged && !!onComment));
  const composer = writing && <form className="diff__compose" onSubmit={event => {
    event.preventDefault();
    if (!canSubmit) return;
    setSaving(true); setProblem(null); setAttemptedBody(body);
    void writing.submit(body, writing.anchor).then(saved => {
      if (saved !== false) { setBody(""); setWriting(null); setAttemptedBody(null); }
    }).catch((error: unknown) => setProblem(String(error))).finally(() => setSaving(false));
  }}>
    {anchorChanged && <div className="notice"><strong>{retry ? "Comment anchor changed. Only the original captured request can be retried." : "Comment anchor changed — review and select a current line before sending."}</strong>
      <p>Your draft is retained at {writing.anchor.file_path} · {writing.anchor.side} line {writing.anchor.line_start}.</p>
      <pre>{writing.lineText.slice(0, 4000)}{writing.lineText.length > 4000 ? "\nOriginal line excerpt truncated." : ""}</pre></div>}
    {retry && !saving && <p className="faint">Retry uses the original request, checkout and anchor; it does not submit against changed lines.</p>}
    {!onComment && !retry && <p className="faint">Comment submission is currently unavailable. Your draft is retained.</p>}
    <textarea aria-label={`comment on ${writing.anchor.file_path} ${writing.anchor.side} line ${writing.anchor.line_start}`} value={body}
      onChange={event => { if (!saving) setBody(event.target.value); }} disabled={saving} rows={2} maxLength={32000} />
    {problem && <p className="error" role="alert">{problem}</p>}
    <button type="submit" className="button button--primary" disabled={!canSubmit}>Comment</button>
    <button type="button" className="button" disabled={saving} onClick={() => { setWriting(null); setBody(""); setAttemptedBody(null); setProblem(null); }}>Cancel comment</button>
  </form>;

  return (
    <section className="diff">
      <div className="diff__head">
        <button type="button" className="diff__toggle" aria-label={`${collapsed ? "Expand" : "Collapse"} ${file.path}`} aria-expanded={!collapsed} onClick={progress.toggle}>
          <span aria-hidden="true">{collapsed ? "▸" : "▾"}</span><span className="mono">{file.path}</span>
        </button>
        {file.old_path !== null && <span className="faint mono">was {file.old_path}</span>}
        <span className="status" data-status={file.status === "removed" ? "failed" : "done"}>
          {file.status}
        </span>
        <span className="faint mono">
          +{file.additions} -{file.deletions}
        </span>
        {mine.length > 0 && <span className="faint">{mine.length} comment{mine.length === 1 ? "" : "s"}</span>}
        <label className="diff__viewed"><input type="checkbox" aria-label={`Viewed ${file.path}`} checked={viewed} onChange={event => progress.mark(event.target.checked)} />Viewed</label>
      </div>
      {progress.problem && <p className="faint diff__notice">{progress.problem}</p>}
      {!collapsed && reason && <p className="faint diff__notice">{reason}</p>}

      {/* git decides what is binary, and rendering its bytes as text is how a review
          turns into a screenful of noise. */}
      {!collapsed && file.binary && !reason && <p className="faint diff__notice">Binary file - not shown.</p>}

      {collapsed && !viewed && longest > UNREADABLE_LINE && (
        <div className="diff__collapsed">
          <p className="faint">
            Collapsed: a minified or generated file - its longest line is{" "}
            {longest.toLocaleString()} characters.
          </p>
          <button type="button" className="button" onClick={progress.toggle}>
            Show diff
          </button>
        </div>
      )}

      {anchorChanged && composer}
      {!collapsed && file.hunks.map((hunk) => (
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
                <LineRow line={line} onAdd={onComment && number !== null ? () => {
                  if (saving) return;
                  setAttemptedBody(null);
                  setWriting(writing?.key === key && !anchorChanged ? null : { key, source: progress.source, scope, lineText: line.text,
                    anchor: { file_path: file.path, side, line_start: number, line_end: number }, submit: onComment, replaySafe: retryCapturedRequest });
                } : undefined} />
                {on.map((comment) => (
                  <Thread key={comment.id} comment={comment} onResolve={onResolve} />
                ))}
                {writing?.key === key && !anchorChanged && composer}
              </div>
            );
          })}
        </div>
      ))}
    </section>
  );
}

function LineRow({ line, onAdd }: { line: DiffLine; onAdd?: () => void }) {
  return (
    <div className="diff__line" data-kind={line.kind}>
      <span className="diff__num mono">{line.old ?? ""}</span>
      <span className="diff__num mono">{line.new ?? ""}</span>
      {onAdd ? <button
        type="button"
        className="diff__add"
        aria-label={`comment on ${line.kind === "removed" ? "old" : "new"} line ${
          line.kind === "removed" ? line.old : line.new
        }`}
        onClick={onAdd}
      >
        +
      </button> : <span />}
      <code className="diff__text">{line.text === "" ? " " : line.text}</code>
    </div>
  );
}

function Thread({
  comment,
  onResolve,
}: {
  comment: Comment;
  onResolve?: (id: number) => Promise<void>;
}) {
  return (
    <div className="diff__comment" data-status={comment.status}>
      <div className="card__row">
        <span className="faint">{comment.author}</span>
        {comment.status === "open" && onResolve ? (
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
