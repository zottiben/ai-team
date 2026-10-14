import { useLayoutEffect, useRef, useState } from "react";
import { Led } from "../../../primitives/Led";
import { signalStyle } from "../../../primitives/signal";
import { useMeasuredRows } from "../../../virtual/useMeasuredRows";
import type { ToolActivityRow } from "./rows";

export const LANES = ["think", "read", "edit", "bash", "mcp"] as const;
export const LANE_NAMES = { think: "Think", read: "Read", edit: "Edit", bash: "Bash", mcp: "MCP" };
export type Disclosure = { opened: ReadonlySet<string>; toggle: (ids: string[], open: boolean) => void; selected?: number | null };

export function ToolRow({ row, disclosure }: { row: ToolActivityRow; disclosure: Disclosure }) {
  const event = row.start ?? row.result;
  if (!event) return null;
  const tool = row.start?.tool ?? row.result?.tool;
  const output = row.result?.tool?.output;
  const result = row.result?.tool?.outcome;
  const status = result === "failed" ? "Failed" : result === "returned" ? "Returned" : row.result ? "Result status unknown" : "Return not recorded";
  const correlation = { paired: "", start_only: "", result_only: "Start not recorded", missing_identity: "Unpaired · identity unavailable", ambiguous: "Unpaired · ambiguous identity" }[row.correlation];
  const time = row.duration ? `${row.duration.milliseconds.toLocaleString(undefined, { maximumFractionDigits: 1 })} ms${row.duration.source === "observed" ? " observed" : " runtime"}` : "Timing unavailable";
  return <div className="synth-tracker-tool" data-result={result ?? "unknown"}>
    <span className="synth-tracker-bracket" data-edge={row.parallel ? row.parallel.index === 0 ? "first" : row.parallel.index === row.parallel.count - 1 ? "last" : "middle" : undefined} aria-hidden="true" />
    <span className="synth-tracker-number" title={`Event #${event.id}`}>{event.id}</span>
    <span className="synth-tracker-lane" style={signalStyle(row.lane ?? "info")}>{row.lane ? LANE_NAMES[row.lane] : "Tool"}</span>
    <div className="synth-tracker-target">
      <span className="synth-mono">{tool?.target?.text ?? event.summary}{tool?.target?.truncated && "… (truncated)"}</span>
      <small>{status}{correlation && ` · ${correlation}`}{row.parallel?.index === 0 && ` · ∥ ${row.parallel.count} observed overlapping calls`}</small>
      {output && <details open={disclosure.opened.has(`output:${event.id}`)} onToggle={e => disclosure.toggle([`output:${event.id}`], e.currentTarget.open)}>
        <summary>Completed output{output.truncated ? " · tail truncated" : ""}</summary>
        <pre className="synth-tracker-output">{output.text}</pre>
      </details>}
      {tool?.name && <small>{tool.name} · node {event.node_run_id ?? "unknown"}</small>}
      <details open={disclosure.opened.has(`evidence:${event.id}`)} onToggle={e => disclosure.toggle([`evidence:${event.id}`], e.currentTarget.open)}>
        <summary>Recorded evidence</summary>
        {[row.start, row.result].map(item => item && <p key={item.id}>#{item.id} · {item.actor ?? "unknown actor"} · {item.summary}<br/>Recorded {item.at} (coarse) · session {item.session_id ?? "unknown"}{item.observation && <><br/>Received {item.observation.at}</>}</p>)}
      </details>
    </div>
    <span className="synth-tracker-meta" title={row.duration?.source === "observed" ? "Host-observed receive interval, not execution duration" : row.duration ? "Pi-reported execution duration" : "No precise timing was recorded"}>{time}{row.lane === "edit" && <small>Edit counts unavailable</small>}</span>
    <Led state={result === "returned" || result === "failed" ? "on" : "off"} signal={result === "failed" ? "attention" : "done"}/>
  </div>;
}

const keyOf = (row: ToolActivityRow) => row.id;
/** A large consecutive read group must not defeat the outer virtualizer. */
export function ReadGroup({ rows, disclosure }: { rows: ToolActivityRow[]; disclosure: Disclosure }) {
  const scroll = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState<number | null>(null);
  const reached = useRef<number | null>(null);
  const { virtualizer, entries } = useMeasuredRows(rows, keyOf, scroll, rows.length > 200, focused);
  useLayoutEffect(() => {
    if (disclosure.selected === null || disclosure.selected === undefined) { reached.current = null; return; }
    const index = rows.findIndex(row => row.id === disclosure.selected);
    if (index < 0 || reached.current === disclosure.selected) return;
    if (rows.length > 200) virtualizer.scrollToIndex(index, { align: "start" });
    else {
      const item = scroll.current?.querySelector<HTMLElement>(`[data-read-id="${disclosure.selected}"]`);
      if (item && scroll.current) scroll.current.scrollTop += item.getBoundingClientRect().top - scroll.current.getBoundingClientRect().top;
    }
    const target = scroll.current?.querySelector<HTMLElement>(`[data-read-id="${disclosure.selected}"]`);
    if (target) { target.focus({ preventScroll: true }); reached.current = disclosure.selected; }
  }, [rows, disclosure.selected, virtualizer, entries]);
  return <div ref={scroll} className="synth-tracker-group-scroll" data-windowed={rows.length > 200 || undefined} tabIndex={0} aria-label="Grouped read operations">
    <div style={rows.length > 200 ? { height: virtualizer.getTotalSize(), position: "relative" } : undefined}>
      {entries.map(item => { const row = rows[item.index]; return row && <div key={item.key} data-index={item.index} data-read-id={row.id} tabIndex={-1} data-selected={row.id === disclosure.selected || undefined} ref={rows.length > 200 ? virtualizer.measureElement : undefined}
        onFocusCapture={() => setFocused(row.id)} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null); }}
        style={item.start === undefined ? undefined : { position: "absolute", width: "100%", transform: `translateY(${item.start}px)` }}>
        <ToolRow row={row} disclosure={disclosure}/>
      </div>; })}
    </div>
  </div>;
}
