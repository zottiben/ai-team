import { useEffect, useRef, useState, type RefObject } from "react";
import "../Shell/shell.css";

const KEY = "ui.synth.panel-width", DEFAULT = 400, MIN = 360, MAIN = 320, HANDLE = 12;
export function panelBounds(preference: number, viewport: number, available: number) {
  const max = Math.max(MIN, Math.floor(Math.min(viewport * .6, available - MAIN - HANDLE)));
  return { width: Math.round(Math.min(max, Math.max(MIN, Number.isFinite(preference) ? preference : DEFAULT))), max,
    stacked: viewport < 900 || available < MIN + MAIN + HANDLE };
}
function storedWidth() {
  try { const raw = localStorage.getItem(KEY), value = Number(raw); return raw !== null && Number.isFinite(value) && value > 0 ? value : DEFAULT; }
  catch { return DEFAULT; }
}

/** Resizes existing siblings; never owns, reparents or remounts their controllers. */
export default function PanelFrame({ host, active }: { host: RefObject<HTMLDivElement | null>; active: boolean }) {
  const handle = useRef<HTMLDivElement>(null), preference = useRef(storedWidth());
  const drag = useRef<{ id: number; x: number; width: number; saved: number } | null>(null);
  const bounds = useRef(panelBounds(preference.current, window.innerWidth, window.innerWidth));
  const live = useRef(bounds.current.width);
  const [stacked, setStacked] = useState(bounds.current.stacked);
  const draw = (width: number) => {
    live.current = Math.round(Math.min(bounds.current.max, Math.max(MIN, width)));
    host.current?.style.setProperty("--synth-panel-width", `${live.current}px`);
    handle.current?.setAttribute("aria-valuenow", String(live.current));
    handle.current?.setAttribute("aria-valuetext", `${live.current} pixels`);
    handle.current?.setAttribute("aria-valuemax", String(bounds.current.max));
  };
  const release = () => {
    const current = drag.current; drag.current = null;
    if (current && handle.current?.hasPointerCapture(current.id)) handle.current.releasePointerCapture(current.id);
  };
  const cancel = () => { const saved = drag.current?.saved; release(); if (saved !== undefined) draw(saved); };
  const commit = () => {
    preference.current = live.current;
    try { localStorage.setItem(KEY, String(live.current)); }
    catch { /* Resizing still works when preference storage is unavailable. */ }
  };
  useEffect(() => {
    const root = host.current;
    if (!root || !active) return;
    const measure = () => {
      bounds.current = panelBounds(preference.current, window.innerWidth, root.getBoundingClientRect().width || window.innerWidth);
      root.dataset.synthStacked = String(bounds.current.stacked);
      setStacked(bounds.current.stacked);
      if (bounds.current.stacked) release();
      draw(drag.current ? live.current : bounds.current.width);
    };
    measure();
    const observer = new ResizeObserver(measure); observer.observe(root);
    window.addEventListener("resize", measure);
    window.addEventListener("blur", cancel);
    return () => {
      release(); observer.disconnect(); window.removeEventListener("resize", measure); window.removeEventListener("blur", cancel);
      root.style.removeProperty("--synth-panel-width"); delete root.dataset.synthStacked;
    };
  }, [host, active]);
  return <div ref={handle} className="synth-panel-resizer" role="separator" aria-label="Resize chat panel" aria-orientation="vertical"
    aria-controls="chat-context" aria-valuemin={MIN} tabIndex={0} hidden={!active || stacked}
    title="Drag to resize. Left widens, Right narrows; Home/End set bounds; Escape cancels a drag."
    onPointerDown={event => {
      if (!active || stacked || event.button !== 0 || drag.current) return;
      event.preventDefault(); event.currentTarget.focus();
      drag.current = { id: event.pointerId, x: event.clientX, width: live.current, saved: live.current };
      event.currentTarget.setPointerCapture(event.pointerId);
    }} onPointerMove={event => {
      const current = drag.current;
      if (current?.id === event.pointerId) draw(current.width + current.x - event.clientX);
    }} onPointerUp={event => { if (drag.current?.id === event.pointerId) { release(); commit(); } }}
    onPointerCancel={event => { if (drag.current?.id === event.pointerId) cancel(); }}
    onLostPointerCapture={event => { if (drag.current?.id === event.pointerId) cancel(); }}
    onDoubleClick={() => { release(); draw(DEFAULT); commit(); }}
    onKeyDown={event => {
      if (event.key === "Escape" && drag.current) { event.preventDefault(); cancel(); return; }
      const step = event.shiftKey ? 50 : 10;
      const next = event.key === "ArrowLeft" ? live.current + step : event.key === "ArrowRight" ? live.current - step
        : event.key === "Home" ? MIN : event.key === "End" ? bounds.current.max : null;
      if (next !== null) { event.preventDefault(); release(); draw(next); commit(); }
    }} />;
}
