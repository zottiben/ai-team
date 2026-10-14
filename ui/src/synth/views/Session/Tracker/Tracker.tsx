import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { RunEvent } from "../../../../api";
import { SynthProvider } from "../../../SynthProvider";
import { useMeasuredRows } from "../../../virtual/useMeasuredRows";
import { trackerRows, type ActivityLane, type TrackerRow } from "./rows";
import { LANES, ReadGroup, ToolRow, type Disclosure } from "./ToolRow";
import { ThinkingRow } from "./ThinkingRow";
const Skin = lazy(() => import("./TrackerSkin"));

export type EventDisclosure = { open: boolean; onToggle: (open: boolean) => void };
const visibleEvent = (event: RunEvent) => event.message !== null || event.thinking.length > 0 || ["tool_call", "tool_result", "failed", "note", "done", "approval_request", "approval_resolved", "build"].includes(event.kind);
function visibleLane(row: TrackerRow, lanes: ReadonlySet<ActivityLane>) {
  if (row.kind === "event") return visibleEvent(row.event);
  if (row.kind === "thinking") return lanes.has("think");
  const tools = row.kind === "reads" ? row.tools : [row];
  return tools.some(tool => tool.result?.tool?.outcome === "failed" || !tool.lane || lanes.has(tool.lane));
}

/** One list owns display/scroll state for BOTH skins. Switching decoration must not
 * replace human/prose DOM, follow-up receipts, or the parent chat's controller. */
export default function Tracker({ events, synth, visible, pattern, eventKey, renderEvent, empty, tail, followRequest = 0 }: {
  events: RunEvent[]; synth: boolean; visible: boolean; pattern: boolean; followRequest?: number;
  eventKey: (event: RunEvent) => string | number;
  renderEvent: (event: RunEvent, disclosure: EventDisclosure) => ReactNode;
  empty: ReactNode; tail: ReactNode;
}) {
  const scroll = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const [newCount, setNewCount] = useState(0);
  const [lanes, setLanes] = useState<ReadonlySet<ActivityLane>>(new Set(LANES));
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const [focused, setFocused] = useState<string | number | null>(null);
  const [jump, setJump] = useState<number | null>(null);
  const [selectedEvent, setSelectedEvent] = useState<number | null>(null);
  const pendingJump = useRef<{ id: number; scrolled: boolean } | null>(null);
  const seenSkin = useRef(synth);
  const holdingJump = useRef(false);
  const seen = useRef<Set<string | number> | null>(null);
  const projected = useMemo(() => trackerRows(events), [events]);
  const allRows = useMemo<TrackerRow[]>(() => synth ? projected : events.filter(visibleEvent).map(event => ({ kind: "event", id: event.id, event })), [events, projected, synth]);
  const rows = useMemo(() => synth ? allRows.flatMap<TrackerRow>(row => {
    if (!visibleLane(row, lanes)) return [];
    if (row.kind !== "reads" || lanes.has("read")) return [row];
    const [first, ...rest] = row.tools.filter(tool => tool.result?.tool?.outcome === "failed");
    return first ? [{ ...row, tools: [first, ...rest] }] : [];
  }) : allRows, [allRows, lanes, synth]);
  const keyOf = useCallback((row: TrackerRow) => row.kind === "event" ? eventKey(row.event) : row.id, [eventKey]);
  const windowed = synth && rows.length > 200;
  const { virtualizer, entries } = useMeasuredRows(rows, keyOf, scroll, windowed, focused);
  const total = virtualizer.getTotalSize();
  const toggle = useCallback((ids: string[], open: boolean) => setOpened(old => {
    if (ids.every(id => old.has(id) === open)) return old;
    const next = new Set(old); ids.forEach(id => open ? next.add(id) : next.delete(id)); return next;
  }), []);
  const disclosure: Disclosure = { opened, toggle, selected: selectedEvent };

  useEffect(() => { holdingJump.current = false; setFollowing(true); setNewCount(0); }, [followRequest]);
  useEffect(() => {
    const keys = new Set(allRows.flatMap(row => row.kind === "reads" ? row.tools.map(keyOf) : row.kind === "thinking" ? row.events.map(eventKey) : [keyOf(row)]));
    const added = seen.current && seenSkin.current === synth ? [...keys].filter(key => !seen.current?.has(key)).length : 0;
    seenSkin.current = synth;
    if (following) setNewCount(0); else if (added) setNewCount(count => count + added);
    seen.current = keys;
  }, [allRows, following, keyOf, eventKey, synth]);
  useLayoutEffect(() => {
    if (visible && following && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [visible, following, rows, total, opened, tail, pattern]);
  useEffect(() => {
    if (jump === null) return;
    const timer = setTimeout(() => { setJump(null); setSelectedEvent(null); pendingJump.current = null; }, 600);
    return () => clearTimeout(timer);
  }, [jump]);
  const reveal = (id: number, event: number) => {
    holdingJump.current = true;
    pendingJump.current = { id, scrolled: false };
    setFollowing(false); setJump(id); setSelectedEvent(event);
    const target = projected.find(row => row.id === id);
    if (target?.kind === "thinking") { setLanes(old => new Set([...old, "think"])); toggle([`think:${event}`], true); }
    if (target?.kind === "reads") setLanes(old => new Set([...old, "read"]));
    if (target?.kind === "tool" && target.lane) { const lane = target.lane; setLanes(old => new Set([...old, lane])); }
  };
  useLayoutEffect(() => {
    const request = pendingJump.current;
    if (!request || !visible) return;
    const index = rows.findIndex(row => row.id === request.id);
    if (index < 0) return;
    if (!request.scrolled) {
      if (windowed) virtualizer.scrollToIndex(index, { align: "center" });
      else {
        const target = list.current?.querySelector<HTMLElement>(`[data-row-id="${request.id}"]`);
        if (target && scroll.current) scroll.current.scrollTop += target.getBoundingClientRect().top - scroll.current.getBoundingClientRect().top;
      }
      request.scrolled = true;
    }
    const target = list.current?.querySelector<HTMLElement>(`[data-row-id="${request.id}"]`);
    if (target && scroll.current) {
      const box = target.getBoundingClientRect(), viewport = scroll.current.getBoundingClientRect();
      // A dynamic row can enter overscan before scrollToIndex finishes reconciling its
      // measured position (notably in WKWebView). Never focus that offscreen estimate.
      if (windowed && (box.bottom <= viewport.top || box.top >= viewport.bottom)) {
        scroll.current.scrollTop += box.top - viewport.top;
        return;
      }
      if (windowed) {
        virtualizer.cancelScroll();
        scroll.current.scrollTop += box.top - viewport.top;
      }
      const precise = target.querySelector<HTMLElement>(`[data-read-id="${selectedEvent}"], [data-thinking-id="${selectedEvent}"]`) ?? target;
      precise.focus({ preventScroll: true }); pendingJump.current = null;
    }
  }, [entries, rows, visible, windowed, virtualizer, selectedEvent]);

  return <SynthProvider><div className="chat-transcript-shell" data-tracker={synth || undefined}>
    {synth && <Suspense fallback={null}><Skin rows={projected} pattern={pattern} lanes={lanes} onLane={lane => setLanes(old => { const next = new Set(old); if (next.has(lane)) next.delete(lane); else next.add(lane); return next; })} onSelect={reveal}/></Suspense>}
    <div ref={scroll} className="chat-transcript" onWheelCapture={() => { holdingJump.current = false; }} onPointerDownCapture={() => { holdingJump.current = false; }} onKeyDownCapture={event => {
      if (["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", " "].includes(event.key)) holdingJump.current = false;
    }} onScroll={() => {
      const host = scroll.current;
      if (host && !holdingJump.current) setFollowing(host.scrollHeight - host.scrollTop - host.clientHeight <= (synth ? 80 : 100));
    }}>
      {!events.length ? empty : <div ref={list} className="chat-messages" aria-label="Conversation messages" data-windowed={windowed || undefined}
        style={windowed ? { height: total, position: "relative" } : undefined}>
        {entries.map(item => {
          const row = rows[item.index]; if (!row) return null;
          return <div key={item.key} data-index={item.index} data-row-id={row.id} data-row-kind={row.kind} data-jump={jump === row.id || undefined}
            className="chat-transcript-row" tabIndex={-1} ref={windowed ? virtualizer.measureElement : undefined}
            onFocusCapture={() => setFocused(keyOf(row))} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null); }}
            style={item.start === undefined ? undefined : { position: "absolute", top: 0, width: "100%", transform: `translateY(${item.start}px)` }}>
            <div className="synth-tracker-message-label" hidden={!synth || row.kind !== "event"}>{row.kind === "event" ? `${row.event.actor === "human" ? "You" : row.event.actor ?? "Notice"}${row.event.node_run_id !== null ? ` · node ${row.event.node_run_id}` : ""}` : null}</div>
            <div className="tracker-content">
              {row.kind === "event" ? renderEvent(row.event, { open: opened.has(`think:${row.id}`), onToggle: open => toggle([`think:${row.id}`], open) })
                : row.kind === "tool" ? <ToolRow row={row} disclosure={disclosure}/>
                  : row.kind === "thinking" ? <ThinkingRow events={row.events} disclosure={disclosure}/>
                    : <><div className="synth-tracker-read-heading">Read ×{row.tools.length} · recorded operations</div><ReadGroup rows={row.tools} disclosure={disclosure}/></>}
              {synth && row.kind === "event" && ["approval_request", "approval_resolved", "build"].includes(row.event.kind) && row.event.message === null && <p>{row.event.summary}</p>}
            </div>
          </div>;
        })}
      </div>}
      {tail}
    </div>
    {!following && visible && <button type="button" className="button chat-follow" onClick={() => { holdingJump.current = false; setFollowing(true); setNewCount(0); }}>
      {synth ? `↓ ${newCount} new` : "Jump to latest"}
    </button>}
    <span className="tracker-announcement" role="status">{synth && !following && newCount > 0 ? `${newCount} new activity rows` : ""}</span>
  </div></SynthProvider>;
}
