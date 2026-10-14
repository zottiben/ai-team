import { useCallback, useMemo, useRef, useState } from "react";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/react-virtual";
import { HwButton } from "../../primitives/HwButton";
import { signalStyle } from "../../primitives/signal";
import { LANES, LANE_NAMES } from "./Tracker/ToolRow";
import type { ActivityLane, TrackerRow } from "./Tracker/rows";

export type PatternEntry = { id: number; row: number; lane: ActivityLane; label: string; duration: number | null };
export function patternEntries(rows: TrackerRow[]): PatternEntry[] {
  return rows.flatMap(row => {
    if (row.kind === "thinking") return row.events.map(event => ({ id: event.id, row: row.id, lane: "think" as const, label: `Thinking summary · event ${event.id}`, duration: null }));
    const tools = row.kind === "reads" ? row.tools : row.kind === "tool" ? [row] : [];
    return tools.flatMap(tool => tool.lane ? [{ id: tool.id, row: row.id, lane: tool.lane,
      label: `${tool.start?.tool?.target?.text ?? tool.start?.summary ?? tool.result?.summary ?? "Tool"} · event ${tool.id}`,
      // Receive intervals are not execute() duration and must not colour duration pads.
      duration: tool.duration?.source === "runtime" ? tool.duration.milliseconds : null }] : []);
  });
}

export function SessionPattern({ rows, onSelect }: { rows: TrackerRow[]; onSelect: (row: number, event: number) => void }) {
  const [limit, setLimit] = useState<48 | 96 | "all">(48);
  const all = useMemo(() => patternEntries(rows), [rows]);
  const offset = limit === "all" ? 0 : Math.max(0, all.length - limit);
  const steps = useMemo(() => all.slice(offset), [all, offset]);
  const scroll = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState<number | null>(null);
  const focusedIndex = useMemo(() => steps.findIndex(step => step.id === focused), [steps, focused]);
  const rangeExtractor = useCallback((range: Parameters<typeof defaultRangeExtractor>[0]) => {
    const normal = defaultRangeExtractor(range);
    return focusedIndex < 0 || normal.includes(focusedIndex) ? normal : [...normal, focusedIndex].sort((a, b) => a - b);
  }, [focusedIndex]);
  const windowed = steps.length > 200;
  const virtual = useVirtualizer<HTMLDivElement, HTMLDivElement>({ horizontal: true, count: steps.length,
    getScrollElement: () => scroll.current, estimateSize: () => 44, getItemKey: index => steps[index]?.id ?? index, overscan: 4, rangeExtractor, enabled: windowed });
  const columns = windowed ? virtual.getVirtualItems().map(item => ({ index: item.index, start: item.start })) : steps.map((_, index) => ({ index, start: index * 44 }));
  const longest = useMemo(() => steps.reduce((max, step) => Math.max(max, step.duration ?? 0), 0), [steps]);
  return <section className="synth-session-pattern" aria-label="Session pattern">
    <div className="synth-pattern-heading"><span className="synth-silk">Session pattern · {steps.length} of {all.length} mapped steps</span>
      <div role="group" aria-label="Pattern range">{([48, 96, "all"] as const).map(value => <HwButton key={value} latched={limit === value} onClick={() => setLimit(value)}>{value === "all" ? "All" : value}</HwButton>)}</div>
    </div>
    <div className="synth-sequence">
      <div className="synth-sequence-labels" aria-hidden="true"><span/>{LANES.map(lane => <span key={lane}>{LANE_NAMES[lane]}</span>)}</div>
      <div ref={scroll} className="synth-sequence-scroll" tabIndex={0} aria-label="Sequence steps">
        <div style={{ width: steps.length * 44, height: "var(--synth-sequence-height)", position: "relative" }}>
          {columns.map(column => { const step = steps[column.index]; if (!step) return null;
            const opacity = step.duration === null ? 1 : .55 + .45 * (longest ? Math.log1p(step.duration) / Math.log1p(longest) : 0);
            const label = `${LANE_NAMES[step.lane]} · ${step.label} · ${step.duration === null ? "duration unavailable" : `${step.duration} ms runtime`}`;
            return <div key={step.id} className="synth-sequence-column" onFocusCapture={() => setFocused(step.id)} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null); }} style={{ transform: `translateX(${column.start}px)` }}>
              <span aria-hidden="true">{(offset + column.index + 1) % 8 === 0 ? offset + column.index + 1 : ""}</span>
              {LANES.map(lane => lane === step.lane ? <button type="button" key={lane} className="synth-step" style={signalStyle(lane)} aria-label={label} title={label} onClick={() => onSelect(step.row, step.id)}><span style={{ opacity }} aria-hidden="true"/></button> : <span key={lane} className="synth-step-empty" aria-hidden="true"/>)}
            </div>;
          })}
        </div>
      </div>
    </div>
    <small className="synth-muted">Only recorded, mapped activity. Unknown lanes stay in the tracker; unknown timing is not zero.</small>
  </section>;
}
