import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ratio } from "../motion/ballistics";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";
import { signalStyle, type Signal } from "./signal";

function point(c: number, r: number, degrees: number) {
  const angle = degrees * Math.PI / 180;
  return [c + r * Math.sin(angle), c - r * Math.cos(angle)];
}
export function arc(c: number, r: number, value: number): string {
  const end = -135 + 270 * ratio(value);
  if (end <= -135) return "";
  const [x, y] = point(c, r, -135), [ex, ey] = point(c, r, end);
  return `M${x} ${y} A${r} ${r} 0 ${end + 135 > 180 ? 1 : 0} 1 ${ex} ${ey}`;
}

/** Values are normalized 0…1. onCommit is the sole setting-write boundary. */
export function Knob({ label, value, onCommit, detents, valueText, defaultValue = 0.5,
  size = 52, signal = "edit", disabled = false, readOnly = false, light = false }: {
  label: string; value: number; onCommit?: (value: number) => void;
  detents?: readonly string[]; valueText?: (value: number) => string; defaultValue?: number;
  size?: 40 | 44 | 52 | 68 | 72 | 120; signal?: Signal; disabled?: boolean; readOnly?: boolean; light?: boolean;
}) {
  const host = useRef<HTMLDivElement>(null), path = useRef<SVGPathElement>(null);
  const pointer = useRef<HTMLSpanElement>(null), output = useRef<HTMLSpanElement>(null);
  const labels = useRef<HTMLDivElement>(null);
  const live = useRef(ratio(value)), display = useRef(ratio(value));
  const drag = useRef<{ id: number; y: number; start: number } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const snapMotion = useRef<{ from: number; to: number; elapsed: number } | null>(null);
  const [moving, setMoving] = useState(false);
  const reduced = useReducedMotion();
  const detentKey = JSON.stringify(detents ?? []);
  const steps = Math.max(0, (detents?.length ?? 0) - 1);
  const locked = disabled || readOnly || !onCommit || detents?.length === 1;
  const width = size >= 120 ? 5 : size >= 68 ? 4 : 3;
  const c = size / 2, r = c - width;
  const text = (v: number) => valueText?.(v) ?? detents?.[Math.round(v * steps)] ?? `${Math.round(v * 100)}%`;
  const snapped = (v: number) => steps ? Math.round(ratio(v) * steps) / steps : ratio(v);
  const cancelTimer = () => { if (timer.current !== null) clearTimeout(timer.current); timer.current = null; };
  const paint = (v: number) => {
    display.current = v;
    path.current?.setAttribute("d", arc(c, r, v));
    if (pointer.current) pointer.current.style.transform = `rotate(${-135 + 270 * v}deg)`;
    const semantic = live.current;
    host.current?.setAttribute("aria-valuenow", String(steps ? Math.round(semantic * steps) : Math.round(semantic * 100)));
    host.current?.setAttribute("aria-valuetext", text(semantic));
    if (output.current) output.current.textContent = text(semantic);
    labels.current?.querySelectorAll("span").forEach((el, i) => { el.dataset.active = String(i === Math.round(v * steps)); });
  };
  const latest = useRef({ paint, locked, snapped, onCommit });
  latest.current = { paint, locked, snapped, onCommit };
  const reset = () => {
    cancelTimer();
    const captured = drag.current;
    drag.current = null;
    if (captured && host.current?.hasPointerCapture(captured.id)) host.current.releasePointerCapture(captured.id);
    snapMotion.current = null;
    live.current = ratio(value);
    paint(live.current);
    setMoving(false);
  };
  useLayoutEffect(() => {
    // A controlled parent's acknowledgement must not cancel its own visual snap.
    if (!locked && snapMotion.current?.to === ratio(value)) {
      cancelTimer(); paint(display.current);
    } else reset();
  }, [value, locked, detentKey, size]);
  useEffect(() => () => cancelTimer(), []);
  useEffect(() => {
    if (reduced) { snapMotion.current = null; paint(live.current); setMoving(false); }
  }, [reduced]);
  useTicker(host, (_now, dt) => {
    const snap = snapMotion.current;
    if (snap) {
      snap.elapsed += dt;
      const p = Math.min(1, snap.elapsed / 180);
      paint(snap.from + (snap.to - snap.from) * (1 - Math.pow(1 - p, 3)));
      if (p === 1) { snapMotion.current = null; setMoving(false); return false; }
    } else paint(live.current);
  }, 60, moving);
  const step = (to: number) => {
    if (latest.current.locked || drag.current) return;
    cancelTimer();
    snapMotion.current = null;
    setMoving(false);
    live.current = latest.current.snapped(to);
    latest.current.paint(live.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      if (!latest.current.locked) latest.current.onCommit?.(live.current);
    }, 250);
  };
  const stepRef = useRef(step);
  stepRef.current = step;
  useEffect(() => {
    const node = host.current;
    const wheel = (event: WheelEvent) => {
      if (latest.current.locked || event.deltaY === 0) return;
      event.preventDefault();
      stepRef.current(live.current + (event.deltaY < 0 ? 1 : -1) * (steps ? 1 / steps : 0.01));
    };
    node?.addEventListener("wheel", wheel, { passive: false });
    return () => node?.removeEventListener("wheel", wheel);
  }, [steps]);

  return <div className="synth-knob-control" style={signalStyle(signal)} data-light={light || undefined}>
    <span className="synth-silk">{label}</span>
    <div className="synth-knob-labels" ref={labels}>
      {detents?.map((name, i) => <span key={`${i}:${name}`} data-active={i === Math.round(ratio(value) * steps)}>{name}</span>)}
    </div>
    <div ref={host} className="synth-knob" role={readOnly ? "meter" : "slider"} aria-label={label} aria-orientation={readOnly ? undefined : "vertical"}
      aria-valuemin={0} aria-valuemax={steps || 100} aria-valuenow={steps ? Math.round(ratio(value) * steps) : Math.round(ratio(value) * 100)}
      aria-valuetext={text(ratio(value))} aria-disabled={readOnly ? undefined : locked} tabIndex={locked ? undefined : 0}
      style={{ width: size, height: size }}
      onPointerDown={event => {
        if (locked || event.button !== 0 || drag.current) return;
        event.preventDefault(); cancelTimer(); snapMotion.current = null;
        event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { id: event.pointerId, y: event.clientY, start: live.current };
        setMoving(!reduced);
      }}
      onPointerMove={event => {
        const current = drag.current;
        if (!current || event.pointerId !== current.id) return;
        live.current = ratio(live.current + (current.y - event.clientY) / 200 * (event.shiftKey ? 0.1 : 1));
        current.y = event.clientY;
        if (reduced) paint(live.current);
      }}
      onPointerUp={event => {
        const current = drag.current;
        if (!current || event.pointerId !== current.id) return;
        drag.current = null;
        event.currentTarget.releasePointerCapture(event.pointerId);
        const from = live.current, to = snapped(from);
        live.current = to;
        if (steps && !reduced && from !== to) {
          paint(from);
          snapMotion.current = { from, to, elapsed: 0 }; setMoving(true);
        } else { paint(to); setMoving(false); }
        onCommit?.(to);
      }}
      onPointerCancel={reset} onLostPointerCapture={() => { if (drag.current) reset(); }}
      onDoubleClick={() => { if (!locked) { cancelTimer(); live.current = snapped(defaultValue); paint(live.current); onCommit?.(live.current); } }}
      onKeyDown={event => {
        if (event.key === "Escape" && (drag.current || timer.current !== null)) { event.preventDefault(); reset(); return; }
        if (locked) return;
        const unit = steps ? 1 / steps : 0.01, big = steps ? unit : 0.1;
        const delta: Record<string, number> = { ArrowUp: unit, ArrowRight: unit, ArrowDown: -unit, ArrowLeft: -unit, PageUp: big, PageDown: -big };
        if (event.key in delta || event.key === "Home" || event.key === "End") {
          event.preventDefault();
          step(event.key === "Home" ? 0 : event.key === "End" ? 1 : live.current + (delta[event.key] ?? 0));
        }
      }}>
      <svg width={size} height={size} aria-hidden="true">
        <path className="synth-knob__track" d={arc(c, r, 1)} fill="none" strokeWidth={width} strokeLinecap="round" />
        <path ref={path} className="synth-knob__arc" d={arc(c, r, display.current)} fill="none" strokeWidth={width} strokeLinecap="round" />
      </svg>
      <span className="synth-knob__cap" style={{ inset: Math.round(size * 0.17) }} />
      <span ref={pointer} className="synth-knob__pointer" style={{ inset: Math.round(size * 0.17), transform: `rotate(${-135 + 270 * display.current}deg)` }}><span style={{ height: size * 0.19 }} /></span>
    </div>
    <span ref={output} className="synth-knob__value">{text(live.current)}</span>
  </div>;
}
