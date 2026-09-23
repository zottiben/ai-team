import type { BoardSlice, BoardStatus } from "./api";

export function boardStatusColor(status: BoardStatus): string {
  const name =
    status === "blocked"
      ? "board-blocked"
      : status === "done"
        ? "board-done"
        : status.replace("_", "-");
  return `var(--status-${name}-default)`;
}

/** One ai-planner slice, carrying the one extra fact ai-team knows: its owning seat. */
export function BoardCard({
  slice,
  current,
  dragging,
  onOpen,
  onDragStart,
}: {
  slice: BoardSlice;
  current: boolean;
  dragging: boolean;
  onOpen: () => void;
  onDragStart: () => void;
}) {
  return (
    <article
      className={`board-card${current ? " is-current" : ""}${dragging ? " is-dragging" : ""}`}
      style={{ borderLeftColor: boardStatusColor(slice.status) }}
      aria-current={current ? "true" : undefined}
    >
      <button type="button" className="board-card__open" onClick={onOpen}>
        <span className="board-card__top">
          <span className="board-card__key">{slice.key}</span>
          <span className="board-card__title">{slice.title}</span>
        </span>

        <span className="board-card__foot">
          {slice.claimed_by ? (
            <Chip className="claim" label={slice.claimed_by} title={`Held by ${slice.claimed_by}`} />
          ) : (
            slice.branch && <Chip className="branch" label={slice.branch} title={slice.branch} />
          )}

          {slice.claimed_by && slice.worktree_path && (
            <Chip
              className="branch"
              label={shortPath(slice.worktree_path)}
              title={slice.worktree_path}
            />
          )}

          <Chip
            className={slice.owner === null ? "owner unowned" : "owner"}
            label={
              (slice.crew ?? []).length > 1
                ? (slice.crew ?? []).join(" + ")
                : (slice.owner ?? "unowned")
            }
            title={
              slice.touches.length > 0
                ? slice.touches.join(", ")
                : "No declared path belongs to a seat"
            }
          />

          {slice.pr_url ? (
            <Chip
              className="pr spacer"
              label={prLabel(slice.pr_url)}
              title={slice.pr_url}
            />
          ) : (
            slice.estimate_files !== null && (
              <Chip
                className="spacer"
                label={`~${slice.estimate_files}f`}
                title={`about ${slice.estimate_files} files`}
              />
            )
          )}
        </span>
      </button>
      <span
        className="board-card__drag"
        data-drag-key={slice.key}
        aria-hidden="true"
        title={`Drag ${slice.key} to another status`}
        onMouseDown={(event) => {
          if (event.button !== 0) return;
          // Native HTML drag/drop is unreliable in WKWebView. Plain mouse events keep
          // the gesture in React from press through release, while the board resolves
          // the status column under the pointer.
          event.preventDefault();
          onDragStart();
        }}
      >
        ⠿
      </span>
    </article>
  );
}

function Chip({
  className,
  label,
  title,
}: {
  className: string;
  label: string;
  title: string;
}) {
  return (
    <span className={`board-chip ${className}`} title={title}>
      <span>{label}</span>
    </span>
  );
}

export function shortPath(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join("/")}`;
}

export function prLabel(url: string): string {
  const match = url.match(/\/pull\/(\d+)(?:\/|$)/);
  return match?.[1] ? `PR #${match[1]}` : "PR";
}
