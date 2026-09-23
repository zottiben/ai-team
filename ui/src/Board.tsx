import { useCallback, useEffect, useRef, useState } from "react";

import {
  board as fetchBoard,
  BOARD_COLUMNS,
  BOARD_STATUSES,
  moveSlice,
  type Board as BoardData,
  type BoardSlice,
  type BoardStatus,
} from "./api";
import { BoardCard, boardStatusColor } from "./BoardCard";
import { BoardDrawer } from "./BoardDrawer";

const COLLAPSED_BY_DEFAULT: BoardStatus[] = ["draft", "done", "deferred"];
const COLLAPSED_KEY = "ai-planner.collapsed-columns";

/**
 * The plan, as ai-planner holds it, with ai-team's seat owner added to each card.
 *
 * Seven columns always exist; the three usually empty ones fold into strips. Moves are
 * optimistic because a drag should feel immediate, and rollback is real because the
 * board must never keep asserting a status ai-planner refused.
 */
export function Board({
  project,
  workspace,
  tick,
}: {
  project: string | null;
  workspace?: string | null;
  tick: number;
}) {
  const [data, setData] = useState<BoardData | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [dragging, setDragging] = useState<BoardSlice | null>(null);
  const [over, setOver] = useState<BoardStatus | null>(null);
  const [collapsed, setCollapsed] = useState<BoardStatus[]>(readCollapsed);
  const loadRevision = useRef(0);
  const pendingMoves = useRef(new Map<string, { status: BoardStatus; reason?: string }>());

  const load = useCallback(async () => {
    const revision = ++loadRevision.current;
    if (project === null) {
      setData(null);
      return;
    }
    try {
      const loaded = await fetchBoard(project, workspace);
      if (revision !== loadRevision.current) return;
      setData({
        ...loaded,
        slices: loaded.slices.map((slice) => {
          const pending = pendingMoves.current.get(slice.key);
          return pending === undefined
            ? slice
            : {
                ...slice,
                status: pending.status,
                blocked_reason:
                  pending.status === "blocked" ? (pending.reason ?? slice.blocked_reason) : null,
              };
        }),
      });
      setProblem(null);
    } catch (error: unknown) {
      if (revision !== loadRevision.current) return;
      setData(null);
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project, workspace]);

  useEffect(() => {
    pendingMoves.current.clear();
    loadRevision.current += 1;
  }, [project, workspace]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  useEffect(() => {
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify(collapsed));
    } catch {
      // A disabled or full preference store is not a reason to stop using the board.
    }
  }, [collapsed]);

  const move = async (slice: BoardSlice, status: BoardStatus) => {
    if (project === null || slice.status === status) return;
    let reason: string | undefined;
    if (status === "blocked") {
      const answer = window.prompt(`Why is ${slice.key} blocked?`);
      if (answer === null || answer.trim() === "") return;
      reason = answer.trim();
    }

    const before = data;
    pendingMoves.current.set(slice.key, { status, reason });
    loadRevision.current += 1;
    setData((current) =>
      current === null
        ? null
        : {
            ...current,
            slices: current.slices.map((candidate) =>
              candidate.key === slice.key
                ? {
                    ...candidate,
                    status,
                    blocked_reason:
                      status === "blocked" ? (reason ?? candidate.blocked_reason) : null,
                  }
                : candidate,
            ),
          },
    );
    try {
      await moveSlice(slice.key, { project, workspace, status, reason });
      // Invalidate reads that began before ai-planner committed the move. The optimistic
      // row is already the acknowledged state; the next ordinary tick will refresh the
      // rest of its metadata without letting an older response snap it back.
      loadRevision.current += 1;
      pendingMoves.current.delete(slice.key);
      setProblem(null);
    } catch (error: unknown) {
      // The server remains the truth. Put the card back before showing why.
      loadRevision.current += 1;
      pendingMoves.current.delete(slice.key);
      setData(before);
      const message = error instanceof Error ? error.message : String(error);
      setProblem(message);
      throw error;
    }
  };

  if (project === null) return <p className="empty">Pick a project to see its plan.</p>;
  if (problem !== null && data === null) return <p className="error">{problem}</p>;
  if (data === null) return <p className="empty">Reading the plan…</p>;
  if (data.plan === null) {
    return <p className="empty">{data.next_step ?? "This checkout has no plan yet."}</p>;
  }

  const opened = data.slices.find((slice) => slice.key === openKey) ?? null;
  const currentSliceKey = data.plan.slice;

  return (
    <div
      className="plan-board"
      onMouseMove={(event) => {
        if (dragging === null) return;
        setOver(boardStatusAt(event.target));
      }}
      onMouseUp={(event) => {
        const slice = dragging;
        const status = boardStatusAt(event.target);
        setDragging(null);
        setOver(null);
        if (slice !== null && status !== null) {
          void move(slice, status).catch(() => undefined);
        }
      }}
      onMouseLeave={() => {
        if (dragging === null) return;
        setOver(null);
      }}
    >
      <div className="main__header">
        <div>
          <h2>{data.plan.title}</h2>
          <span className="faint mono">{data.plan.plan}</span>
        </div>
        {problem !== null && <span className="error">{problem}</span>}
      </div>

      <div className="plan-board__columns">
        {BOARD_STATUSES.map((meta) => {
          const slices = data.slices
            .filter((slice) => slice.status === meta.value)
            .sort((left, right) => left.ord - right.ord);
          const isCollapsed = collapsed.includes(meta.value);
          const isTarget = over === meta.value && dragging?.status !== meta.value;

          return (
            <section
              key={meta.value}
              data-board-status={meta.value}
              className={`board-column${isCollapsed ? " is-collapsed" : ""}${isTarget ? " is-target" : ""}`}
              aria-label={`${meta.label}, ${slices.length} slices`}
            >
              <header className="board-column__head">
                <span
                  className="board-status-dot"
                  style={{ background: boardStatusColor(meta.value) }}
                />
                <span className="board-column__title">{meta.label}</span>
                <span className="board-column__count">{slices.length}</span>
                <button
                  type="button"
                  className="board-column__collapse"
                  onClick={() =>
                    setCollapsed((current) =>
                      current.includes(meta.value)
                        ? current.filter((status) => status !== meta.value)
                        : [...current, meta.value],
                    )
                  }
                  aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${meta.label}`}
                  title={isCollapsed ? "Expand" : "Collapse"}
                >
                  {isCollapsed ? "›" : "‹"}
                </button>
              </header>

              {!isCollapsed && (
                <div className="board-column__body">
                  {slices.map((slice) => (
                    <BoardCard
                      key={slice.id || slice.key}
                      slice={slice}
                      current={slice.key === currentSliceKey}
                      dragging={dragging?.key === slice.key}
                      onOpen={() => setOpenKey(slice.key)}
                      onDragStart={() => setDragging(slice)}
                    />
                  ))}
                  {slices.length === 0 && (
                    <p className="board-column__empty">
                      {dragging === null ? "Nothing here." : "Drop here"}
                    </p>
                  )}
                </div>
              )}
            </section>
          );
        })}
      </div>

      {opened !== null && (
        <BoardDrawer
          project={project}
          workspace={workspace}
          slice={opened}
          tick={tick}
          onChanged={() => void load()}
          onMove={move}
          onClose={() => setOpenKey(null)}
        />
      )}
    </div>
  );
}

function boardStatusAt(target: EventTarget | null): BoardStatus | null {
  if (!(target instanceof Element)) return null;
  const status = target.closest<HTMLElement>("[data-board-status]")?.dataset.boardStatus;
  return BOARD_COLUMNS.includes(status as BoardStatus) ? (status as BoardStatus) : null;
}

function readCollapsed(): BoardStatus[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "null") as unknown;
    if (!Array.isArray(parsed)) return COLLAPSED_BY_DEFAULT;
    const valid = new Set(BOARD_COLUMNS);
    return parsed.filter((status): status is BoardStatus =>
      typeof status === "string" && valid.has(status as BoardStatus),
    );
  } catch {
    return COLLAPSED_BY_DEFAULT;
  }
}
