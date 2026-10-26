import { useLayoutEffect, useRef } from "react";
import { advanceVu, ratio, type VuState } from "../motion/ballistics";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";

export function VuMeter({ value, label, segments = 12 }: { value: number; label: string; segments?: 12 | 16 }) {
  const host = useRef<HTMLDivElement>(null), bars = useRef<HTMLSpanElement>(null);
  const state = useRef<VuState>({ level: ratio(value), peak: ratio(value), held: 1 });
  const reduced = useReducedMotion();
  const draw = () => bars.current?.querySelectorAll("i").forEach((bar, i) => {
    const lit = String((i + 1) / segments <= state.current.level);
    const peak = String(state.current.peak > 0 && i === Math.max(0, Math.ceil(state.current.peak * segments) - 1));
    if (bar.dataset.lit !== lit) bar.dataset.lit = lit;
    if (bar.dataset.peak !== peak) bar.dataset.peak = peak;
  });
  useLayoutEffect(() => {
    if (reduced) state.current = { level: ratio(value), peak: ratio(value), held: 1 };
    else state.current = advanceVu(state.current, value, 0);
    draw();
  }, [value, reduced, segments]);
  useTicker(host, (_now, dt) => {
    state.current = advanceVu(state.current, value, dt / 1000);
    draw();
    return state.current.level !== ratio(value) || state.current.peak !== state.current.level;
  }, 20, true, value);
  return <div ref={host} className="synth-meter">
    <span>{label} · {Number.isFinite(value) ? `${Math.round(ratio(value) * 100)}%` : "unavailable"}</span>
    <span ref={bars} className="synth-segments synth-vu" aria-hidden="true">{Array.from({ length: segments }, (_, i) =>
      <i key={i} data-band={(i + 1) / segments >= 0.87 ? "high" : (i + 1) / segments >= 0.7 ? "mid" : "low"} />)}</span>
  </div>;
}
