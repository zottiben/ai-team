import { useLayoutEffect, useRef } from "react";
import { advanceVu, type VuState } from "../../motion/ballistics";
import { useReducedMotion } from "../../motion/useReducedMotion";
import { useTicker } from "../../motion/useTicker";
import "./activity-equalizer.css";

const BANDS = 8, SEGMENTS = 9;
// A decorative musical pattern, never a sample of audio, tokens or tool work.
function level(time: number, band: number) {
  const beat = ((Math.sin(time * 6.7 + band * .75) + 1) / 2) ** 3;
  const ripple = (Math.sin(time * (3.1 + band * .17) + band * 2.3) + 1) / 2;
  return .08 + .75 * beat + .15 * ripple;
}

export default function ActivityEqualizer({ active }: { active: boolean }) {
  const host = useRef<HTMLSpanElement>(null), elapsed = useRef(0);
  const bars = useRef<(SVGGElement | null)[]>([]);
  const state = useRef<VuState[]>([]);
  const reduced = useReducedMotion();
  const draw = () => bars.current.forEach((bar, band) => {
    const value = state.current[band];
    bar?.querySelectorAll("rect").forEach((segment, row) => {
      const lit = String(!!value && row < Math.round(value.level * SEGMENTS));
      const peak = String(!!value && value.peak > 0 && row === Math.ceil(value.peak * SEGMENTS) - 1);
      if (segment.dataset.lit !== lit) segment.dataset.lit = lit;
      if (segment.dataset.peak !== peak) segment.dataset.peak = peak;
    });
  });
  useLayoutEffect(() => {
    elapsed.current = 0;
    state.current = Array.from({ length: BANDS }, (_, band) => {
      const value = active ? level(0, band) : 0;
      return { level: value, peak: value, held: 1 };
    });
    draw();
  }, [active, reduced]);
  useTicker(host, (_now, dt) => {
    const seconds = Math.min(.1, Math.max(0, dt / 1000));
    elapsed.current = (elapsed.current + seconds) % 120;
    state.current = state.current.map((value, band) => advanceVu(value, level(elapsed.current, band), seconds));
    draw();
  }, 20, active);
  return <span ref={host} className="synth-activity-eq" role="img" aria-label={active ? "Chat working" : "Chat not running"}
    data-active={active} title="Decorative equalizer: indicates an active chat, not audio or measured progress.">
    <svg viewBox="0 0 46 29" aria-hidden="true" focusable="false">
      {Array.from({ length: BANDS }, (_, band) => <g key={band} ref={node => { bars.current[band] = node; }}>
        {Array.from({ length: SEGMENTS }, (_, row) => <rect key={row} x={band * 6} y={27 - row * 3} width={4} height={2}
          data-band={row >= 7 ? "high" : row >= 4 ? "mid" : "low"} />)}
      </g>)}
    </svg>
  </span>;
}
