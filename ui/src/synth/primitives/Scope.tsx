import { useLayoutEffect, useRef } from "react";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";
import { useReadout } from "../motion/readouts";
import { ceiling, describeSeries, plot, recent, type Series } from "./graph";
import { Lcd } from "./Lcd";
import { signalStyle, type Signal } from "./signal";

/** read returns an existing snapshot. Rendering never collects or invents samples. */
export function Scope({ label, unit, samples, read, active = false, signal = "edit", window = 120 }: {
  label: string; unit: string; samples: Series; read?: () => Series; active?: boolean; signal?: Signal; window?: 64 | 120;
}) {
  const host = useRef<HTMLDivElement>(null), line = useRef<SVGPathElement>(null), fill = useRef<SVGPathElement>(null);
  const value = useRef<HTMLSpanElement>(null), status = useRef<HTMLSpanElement>(null);
  const reduced = useReducedMotion();
  const snapshot = () => recent(read?.() ?? samples, window);
  const draw = (data: Series) => {
    const paths = plot(data, ceiling(data, "rate"));
    if (line.current?.getAttribute("d") !== paths.line) line.current?.setAttribute("d", paths.line);
    if (fill.current?.getAttribute("d") !== paths.fill) fill.current?.setAttribute("d", paths.fill);
  };
  const numbers = (data: (number | null)[]) => {
    if (value.current) value.current.textContent = describeSeries(data, unit);
    if (status.current) status.current.textContent = data.length === 0 ? "No samples" : data.every(v => v === null) ? "History unavailable"
      : data.some(v => v === null) ? "Partial history" : data.every(v => v === 0) ? "Idle" : `${data.length} samples`;
  };
  useLayoutEffect(() => { const data = snapshot(); draw(data); numbers(data); }, [samples, window, unit, active, reduced]);
  useTicker(host, () => draw(snapshot()), 30, active && !!read);
  useReadout(host, () => {
    const data = snapshot(); numbers(data);
    if (reduced) draw(data);
  }, active && !!read);
  return <Lcd ref={host} grid className="synth-graph" style={signalStyle(signal)}>
    <div className="synth-graph__heading"><span className="synth-silk">{label}</span><span ref={value}>{describeSeries(recent(samples, window), unit)}</span></div>
    <svg viewBox="0 0 300 80" preserveAspectRatio="none" aria-hidden="true">
      <path className="synth-graph__baseline" d="M0 78H300" />
      <path ref={fill} className="synth-graph__fill" /><path ref={line} className="synth-graph__line" vectorEffect="non-scaling-stroke" />
    </svg>
    <span ref={status} className="synth-muted" />
  </Lcd>;
}
