import { useCallback, useEffect, useRef, useState } from "react";

import {
  BOARD_STATUSES,
  boardSlice,
  claimBoardSlice,
  editBoardSlice,
  noteBoardSlice,
  releaseBoardSlice,
  crewLabel,
  type BoardSlice,
  type BoardSliceDetail,
  type BoardStatus,
} from "./api";
import { boardStatusColor, prLabel } from "./BoardCard";
import { BoardMarkdown } from "./BoardMarkdown";

/** The slice opened, using the same scope-first hierarchy as ai-planner's drawer. */
export function BoardDrawer({
  project,
  workspace,
  slice,
  tick,
  onChanged,
  onMove,
  onClose,
}: {
  project: string;
  workspace?: string | null;
  slice: BoardSlice;
  tick: number;
  onChanged: () => void;
  onMove: (slice: BoardSlice, to: BoardStatus) => Promise<void>;
  onClose: () => void;
}) {
  const [detail, setDetail] = useState<BoardSliceDetail | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const panel = useRef<HTMLElement>(null);

  const load = useCallback(async () => {
    try {
      setDetail(await boardSlice(project, slice.key, workspace));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project, slice.key, workspace]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);

  useEffect(() => {
    panel.current?.focus();
  }, [slice.key]);

  const current = detail?.slice ?? slice;

  const act = async (run: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await run();
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      await load();
      onChanged();
    }
  };

  return (
    <>
      <div className="board-scrim" onClick={onClose} aria-hidden="true" />
      <aside
        className="board-drawer"
        ref={panel}
        tabIndex={-1}
        role="dialog"
        aria-label={`${current.key}: ${current.title}`}
      >
        <header className="board-drawer__head">
          <div className="board-drawer__head-row">
            <span className="board-card__key">{current.key}</span>
            <label
              className="board-status-select"
              style={{ color: boardStatusColor(current.status) }}
            >
              <span
                className="board-status-dot"
                style={{ background: boardStatusColor(current.status) }}
              />
              <select
                value={current.status}
                disabled={busy}
                aria-label="Status"
                onChange={(event) => {
                  const status = event.target.value as BoardStatus;
                  void act(() => onMove(current, status));
                }}
              >
                {BOARD_STATUSES.map((status) => (
                  <option key={status.value} value={status.value}>
                    {status.label}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="board-drawer__close"
              onClick={onClose}
              aria-label="Close"
              title="Close (Esc)"
            >
              ✕
            </button>
          </div>
          <h2>{current.title}</h2>

          <div className="board-drawer__actions">
            {current.claimed_by ? (
              <button
                type="button"
                className="button"
                disabled={busy}
                onClick={() => void act(() => releaseBoardSlice(project, current.key, workspace))}
              >
                Release
              </button>
            ) : (
              <button
                type="button"
                className="button button--primary"
                disabled={busy}
                onClick={() => void act(() => claimBoardSlice(project, current.key, workspace))}
              >
                Claim
              </button>
            )}
            <button
              type="button"
              className="button"
              disabled={busy}
              onClick={() => {
                const url = window.prompt("Pull request URL", current.pr_url ?? "");
                if (url === null) return;
                void act(() => editBoardSlice(project, current.key, url.trim(), workspace));
              }}
            >
              {current.pr_url ? "Change PR link" : "Link a PR"}
            </button>
          </div>
        </header>

        <div className="board-drawer__body">
          {problem !== null && <p className="error">{problem}</p>}

          {current.blocked_reason && (
            <div className="board-callout blocked">
              <b>Blocked</b>
              <span>{current.blocked_reason}</span>
            </div>
          )}

          {current.claimed_by && (
            <div className="board-callout">
              <b>Held by {current.claimed_by}</b>
              <span>
                {current.worktree_path}
                {current.claimed_at && ` · since ${ago(current.claimed_at)}`}
              </span>
            </div>
          )}

          <div className={`board-callout${current.owner === null ? " unowned" : ""}`}>
            <b>
              {crewLabel(current) === null
                ? "No seat owns this slice"
                : (current.crew ?? []).length > 1
                  ? `Built by ${crewLabel(current)}`
                  : `Owned by ${crewLabel(current)}`}
            </b>
            <span>{current.touches.length > 0 ? current.touches.join(", ") : "No paths declared"}</span>
          </div>

          <Field label="Scope">
            {current.scope_md?.trim() ? (
              <BoardMarkdown source={current.scope_md} />
            ) : (
              <p className="faint">Not written yet.</p>
            )}
          </Field>

          {current.demo_md && (
            <Field label="How to prove it works">
              <BoardMarkdown source={current.demo_md} />
            </Field>
          )}

          <Field label="Delivery">
            <dl className="board-facts">
              <Fact label="Branch" mono>{current.branch}</Fact>
              <Fact label="Base" mono>{current.base_branch}</Fact>
              <Fact label="Pull request">
                {current.pr_url ? (
                  <a href={current.pr_url} target="_blank" rel="noreferrer noopener">
                    {prLabel(current.pr_url)}
                  </a>
                ) : null}
              </Fact>
              <Fact label="Estimate">
                {current.estimate_files === null ? null : `${current.estimate_files} files`}
              </Fact>
              <Fact label="Started" title={exact(current.started_at)}>{ago(current.started_at)}</Fact>
              <Fact label="Completed" title={exact(current.completed_at)}>
                {ago(current.completed_at)}
              </Fact>
            </dl>
          </Field>

          <Field label={`Progress${detail ? ` (${detail.log.length})` : ""}`}>
            <NoteBox
              busy={busy}
              onSubmit={(body) => act(() => noteBoardSlice(project, current.key, body, workspace))}
            />
            {detail?.log.length === 0 && <p className="faint">Nothing recorded yet.</p>}
            <ol className="board-log">
              {detail?.log.map((entry) => (
                <li key={entry.id} className={`board-log__entry kind-${entry.kind}`}>
                  <div className="board-log__meta">
                    <span>{entry.kind}</span>
                    <span title={exact(entry.at)}>{ago(entry.at)}</span>
                    {entry.actor && <span>· {entry.actor}</span>}
                  </div>
                  <BoardMarkdown source={entry.body} tight />
                </li>
              ))}
            </ol>
          </Field>
        </div>
      </aside>
    </>
  );
}

function NoteBox({
  busy,
  onSubmit,
}: {
  busy: boolean;
  onSubmit: (body: string) => Promise<void>;
}) {
  const [body, setBody] = useState("");
  const send = async () => {
    const text = body.trim();
    if (text === "" || busy) return;
    await onSubmit(text);
    setBody("");
  };

  return (
    <div className="board-note">
      <textarea
        value={body}
        rows={2}
        placeholder="Record what happened…"
        aria-label="Progress note"
        disabled={busy}
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void send();
          }
        }}
      />
      {body.trim() !== "" && (
        <div className="board-note__foot">
          <span className="faint">⌘↵ to save</span>
          <button
            type="button"
            className="button button--primary"
            disabled={busy}
            onClick={() => void send()}
          >
            Save note
          </button>
        </div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <section className="board-field">
      <h3>{label}</h3>
      {children}
    </section>
  );
}

function Fact({
  label,
  children,
  mono = false,
  title,
}: {
  label: string;
  children: React.ReactNode;
  mono?: boolean;
  title?: string;
}) {
  if (children === null || children === undefined || children === "") return null;
  return (
    <>
      <dt>{label}</dt>
      <dd className={mono ? "mono" : undefined} title={title}>
        {children}
      </dd>
    </>
  );
}

function ago(value: string | null): string | null {
  if (value === null) return null;
  const elapsed = Date.now() - new Date(value).getTime();
  if (!Number.isFinite(elapsed)) return value;
  const seconds = Math.max(0, Math.floor(elapsed / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function exact(value: string | null): string | undefined {
  if (value === null) return undefined;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}
