import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { RunEvent } from "../../../../api";
import { useMeasuredRows } from "../../../virtual/useMeasuredRows";
import type { Disclosure } from "./ToolRow";

const keyOf = (item: { key: string }) => item.key;
function Summaries({ events, selected }: { events: RunEvent[]; selected?: number | null }) {
  const items = useMemo(() => events.flatMap(event => event.thinking.map((text, index) => ({ key: `${event.id}:${index}`, eventId: event.id, text }))), [events]);
  const scroll = useRef<HTMLDivElement>(null);
  const reached = useRef<number | null>(null);
  const [focused, setFocused] = useState<string | null>(null);
  const windowed = items.length > 200;
  const { virtualizer, entries } = useMeasuredRows(items, keyOf, scroll, windowed, focused);
  useLayoutEffect(() => {
    if (selected === null || selected === undefined) { reached.current = null; return; }
    const index = items.findIndex(item => item.key === `${selected}:0`);
    if (index < 0 || reached.current === selected) return;
    if (windowed) virtualizer.scrollToIndex(index, { align: "start" });
    const target = scroll.current?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    if (target && scroll.current) {
      if (!windowed) scroll.current.scrollTop += target.getBoundingClientRect().top - scroll.current.getBoundingClientRect().top;
      target.focus({ preventScroll: true }); reached.current = selected;
    }
  }, [selected, items, virtualizer, windowed, entries]);
  return <div ref={scroll} className="synth-tracker-group-scroll synth-tracker-output" tabIndex={0} aria-label="Thinking summaries">
    <div style={windowed ? { height: virtualizer.getTotalSize(), position: "relative" } : undefined}>
      {entries.map(item => <div key={item.key} data-index={item.index} data-thinking-id={items[item.index]?.eventId} tabIndex={-1}
        onFocusCapture={() => setFocused(String(item.key))} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null); }} ref={windowed ? virtualizer.measureElement : undefined}
        style={item.start === undefined ? undefined : { position: "absolute", width: "100%", transform: `translateY(${item.start}px)` }}>
        <p>{items[item.index]?.text}</p>
      </div>)}
    </div>
  </div>;
}
export function ThinkingRow({ events, disclosure }: { events: RunEvent[]; disclosure: Disclosure }) {
  const open = events.some(event => disclosure.opened.has(`think:${event.id}`));
  const count = events.reduce((sum, event) => sum + event.thinking.length, 0);
  const { toggle } = disclosure;
  useEffect(() => { if (open) toggle(events.map(event => `think:${event.id}`), true); }, [events, open, toggle]);
  return <details className="synth-tracker-thinking" open={open} onToggle={e => disclosure.toggle(events.map(event => `think:${event.id}`), e.currentTarget.open)}>
    <summary><span className="synth-tracker-lane">Think</span> Thinking summary{count > 1 && ` ×${count}`} <span className="synth-tracker-meta">Timing unavailable</span></summary>
    {open && <Summaries events={events} selected={disclosure.selected}/>}
  </details>;
}
