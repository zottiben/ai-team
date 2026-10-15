import type { RefObject } from "react";
import { BoardMarkdown } from "../../../BoardMarkdown";
import { HwButton } from "../../primitives/HwButton";
import { Led } from "../../primitives/Led";
import { counters, durationLabel, durationMs, matches, runSignal, type RunFilter, type RunRecord } from "./runs";
import type { WorkView } from "./view";

const FILTERS: readonly { id: RunFilter; label: string }[] = [
  { id: "all", label: "All" }, { id: "done", label: "Done" },
  { id: "cancelled", label: "Cancelled" }, { id: "other", label: "Other" },
];
/** The spec clamps a long prompt to four lines rather than three (§9.2). */
const LONG = 220;

/**
 * The turn rows (§9.2), oldest first with the live run at the bottom.
 *
 * The filter keeps every state reachable: `Other` is everything Done and Cancelled would
 * hide, counted, so a failed or interrupted run is never silently absent.
 */
export function TurnsList({ records, now, view, pulse, listRef, onScroll, onReadIntent }: {
  records: readonly RunRecord[]; now: number; view: WorkView; pulse: number | null;
  listRef: RefObject<HTMLOListElement | null>; onScroll: () => void; onReadIntent: () => void;
}) {
  const totals = counters(records);
  const count = (filter: RunFilter) => filter === "all" ? totals.runs
    : filter === "done" ? totals.done
      : filter === "cancelled" ? totals.cancelled
        : totals.live + totals.others.reduce((sum, other) => sum + other.count, 0);
  const visible = records.filter(record => matches(record, view.filter));
  return <>
    <div className="synth-turns__filters" role="group" aria-label="Filter turns">
      {FILTERS.map(filter => <HwButton key={filter.id} latched={view.filter === filter.id}
        onClick={() => view.setFilter(filter.id)}>{filter.label} {count(filter.id)}</HwButton>)}
    </div>
    {/* Focusable because it scrolls: a reader with no pointer has to be able to move it. */}
    <ol ref={listRef} className="synth-turns" aria-label="Turns, oldest first" tabIndex={0}
      onScroll={onScroll} onWheelCapture={onReadIntent} onPointerDownCapture={onReadIntent}
      onKeyDownCapture={event => { if (["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", " "].includes(event.key)) onReadIntent(); }}>
      {visible.map(record => {
        const open = view.expanded.has(record.runId);
        const expandable = record.prompt.length > LONG || record.prompt.includes("\n");
        const ms = durationMs(record, now);
        return <li key={record.runId} className="synth-turn" data-run-id={record.runId} data-kind={record.kind}
          data-status={record.status} data-pulse={pulse === record.runId || undefined} tabIndex={-1}>
          <span className="synth-turn__number synth-mono">#{record.runId}</span>
          <div className="synth-turn__body">
            {open
              ? <div className="synth-turn__prompt-full"><BoardMarkdown source={record.prompt} /></div>
              : <p className="synth-turn__prompt" data-clamp={record.prompt.length > LONG ? 4 : 3}
                data-continuation={record.continuation || undefined}>{record.prompt}</p>}
            <p className="synth-turn__meta">
              <span className="synth-mono">{record.provider}/{record.model} · {durationLabel(ms)}</span>
              {expandable && <button type="button" className="synth-turn__more"
                onClick={() => view.toggleExpanded(record.runId)}>{open ? "Show less" : "Show full prompt"}</button>}
            </p>
            {(record.attempt > 1 || record.reasoning || record.sliceKey) && <p className="synth-turn__meta synth-mono">
              {[record.attempt > 1 ? `attempt ${record.attempt}` : null, record.reasoning ? `thinking ${record.reasoning}` : null, record.sliceKey]
                .filter(Boolean).join(" · ")}
            </p>}
            {record.blockedReason && <p className="synth-turn__reason">{record.blockedReason}</p>}
          </div>
          <span className="synth-turn__status">
            <Led state={record.live ? "pulse" : record.kind === "done" ? "on" : "off"} signal={runSignal(record)} />
            <span className="synth-silk">{record.label}</span>
          </span>
        </li>;
      })}
    </ol>
    {!records.length && <p className="synth-muted">No turns yet.</p>}
    {records.length > 0 && !visible.length && <p className="synth-muted">No runs in this filter. {totals.runs} recorded.</p>}
  </>;
}
