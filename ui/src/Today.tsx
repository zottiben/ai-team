import { useCallback, useEffect, useState } from "react";

import { today as fetchToday, type TodayItem } from "./api";

/** What each tier means, said once, where a reader can see it. */
const URGENCY: Record<TodayItem["urgency"], { label: string; why: string; status: string }> = {
  blocking: { label: "Blocking", why: "a run is parked and you are the reason", status: "parked" },
  overdue: { label: "Overdue", why: "its time has already passed", status: "failed" },
  failed: { label: "Failed", why: "it will not un-fail on its own", status: "failed" },
  review: { label: "Review", why: "finished work waiting to be looked at", status: "done" },
  question: { label: "Question", why: "the plan is waiting on an answer", status: "blocked" },
  due: { label: "Due", why: "due about now", status: "queued" },
  in_flight: { label: "In flight", why: "under way; nothing for you to do", status: "running" },
};

/**
 * Today: what to work on now, across every project.
 *
 * The order comes from the server and is not re-sorted here - the ranking is one
 * judgement, tested in core, and a second one in the client would be a second answer to
 * the same question.
 */
export function Today({ tick, onOpenRun }: { tick: number; onOpenRun: (id: number) => void }) {
  const [items, setItems] = useState<TodayItem[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setItems(await fetchToday());
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, tick]);

  if (problem !== null) return <p className="error">{problem}</p>;
  if (items === null) return <p className="empty">Looking…</p>;

  // Nothing waiting is a real answer, and worth saying warmly rather than as a blank.
  if (items.length === 0) {
    return <p className="empty">Nothing is waiting on you. The board is where work starts.</p>;
  }

  const top = items[0];

  return (
    <div className="today">
      <div className="main__header">
        <h2>Today</h2>
        <span className="faint">{items.length} thing(s)</span>
      </div>

      {top !== undefined && (
        // Called out on its own: a ranked list whose first item looks like all the others
        // is a list people read top to bottom anyway.
        <div className="today__first">
          <span className="dock__title">Do this first</span>
          <Row item={top} onOpenRun={onOpenRun} />
          <span className="faint">{URGENCY[top.urgency].why}</span>
        </div>
      )}

      <div className="list">
        {items.slice(1).map((item, index) => (
          <Row key={`${item.kind}-${item.title}-${index}`} item={item} onOpenRun={onOpenRun} />
        ))}
      </div>
    </div>
  );
}

function Row({ item, onOpenRun }: { item: TodayItem; onOpenRun: (id: number) => void }) {
  const tier = URGENCY[item.urgency];
  const body = (
    <>
      <div className="card__row">
        <span className="status" data-status={tier.status}>
          {tier.label}
        </span>
        <span className="faint mono">{item.project ?? ""}</span>
      </div>
      <span>{item.title}</span>
      {item.detail !== null && <span className="faint">{item.detail}</span>}
    </>
  );

  // Only rows that point at a run are clickable. A button that does nothing is worse
  // than plain text, because it invites a click and then ignores it.
  return item.run_id === null ? (
    <div className="card">{body}</div>
  ) : (
    <button type="button" className="card" onClick={() => onOpenRun(item.run_id ?? 0)}>
      {body}
    </button>
  );
}
