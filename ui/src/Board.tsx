import { useCallback, useEffect, useState } from "react";

import { board as fetchBoard, BOARD_COLUMNS, moveSlice, type Board as BoardData } from "./api";

/**
 * The plan, as ai-planner holds it.
 *
 * Nothing here is ai-team's own copy: the slices are read from ai-planner on every load
 * and moves are written straight back, so `aip status` in a terminal and this board are
 * the same state rather than two that agree until they do not.
 *
 * The one thing ai-team adds is the owner - which seat's zone covers the paths a slice
 * declared. That is ai-team's question, not the plan's, and it is why the card can say
 * who would build it.
 */
export function Board({ project, tick }: { project: string | null; tick: number }) {
  const [data, setData] = useState<BoardData | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (project === null) {
      setData(null);
      return;
    }
    try {
      setData(await fetchBoard(project));
      setProblem(null);
    } catch (error: unknown) {
      setData(null);
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [project]);

  useEffect(() => {
    void load();
  }, [load, tick]);

  const move = async (key: string, status: string) => {
    if (project === null) return;
    setBusy(key);
    try {
      // Blocking without saying why leaves the next session guessing, and ai-planner
      // asks for a reason precisely so it does not have to.
      const reason = status === "blocked" ? "moved on the ai-team board" : undefined;
      await moveSlice(key, { project, status, reason });
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  };

  if (project === null) {
    return <p className="empty">Pick a project to see its plan.</p>;
  }
  if (problem !== null) {
    return <p className="error">{problem}</p>;
  }
  if (data === null) {
    return <p className="empty">Reading the plan…</p>;
  }

  // Every project starts here: a checkout nobody has planned in yet. Said as the next
  // step rather than as an empty board, which reads as something having gone wrong.
  if (data.plan === null) {
    return <p className="empty">{data.next_step ?? "This checkout has no plan yet."}</p>;
  }

  return (
    <div className="board">
      <div className="main__header">
        <h2>{data.plan.title}</h2>
        <span className="faint mono">{data.plan.plan}</span>
      </div>

      <div className="board__columns">
        {BOARD_COLUMNS.map((column) => {
          const slices = data.slices.filter((slice) => slice.status === column);
          // Empty columns stay: they are the shape of the workflow, and a board whose
          // columns move about as work flows is a board you have to re-read every time.
          return (
            <section key={column} className="board__column" aria-label={column}>
              <div className="board__column-head">
                <span className="status" data-status={column}>
                  {column.replace("_", " ")}
                </span>
                <span className="nav-item__count">{slices.length || ""}</span>
              </div>
              {slices.map((slice) => (
                <article key={slice.key} className="board__card">
                  <div className="card__row">
                    <span className="mono">{slice.key}</span>
                    {slice.owner === null ? (
                      // Worth saying out loud: a slice nobody owns will be reported
                      // undone rather than handed to somebody.
                      <span className="faint" title={slice.touches.join(", ")}>
                        unowned
                      </span>
                    ) : (
                      <span className="faint" title={slice.touches.join(", ")}>
                        {slice.owner}
                      </span>
                    )}
                  </div>
                  <span>{slice.title}</span>
                  {slice.claimed_by !== null && (
                    <span className="faint mono">claimed by {slice.claimed_by}</span>
                  )}
                  <label className="board__move">
                    <span className="faint">move to</span>
                    <select
                      value={slice.status}
                      disabled={busy === slice.key}
                      aria-label={`move ${slice.key}`}
                      onChange={(event) => void move(slice.key, event.target.value)}
                    >
                      {BOARD_COLUMNS.map((option) => (
                        <option key={option} value={option}>
                          {option.replace("_", " ")}
                        </option>
                      ))}
                    </select>
                  </label>
                </article>
              ))}
            </section>
          );
        })}
      </div>
    </div>
  );
}
