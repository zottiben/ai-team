import { useLayoutEffect, useRef } from "react";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";
import { useReadout } from "../motion/readouts";
import { ceiling, describeSeries, plot, recent, type Series } from "./graph";
import { Lcd } from "./Lcd";
import { signalStyle, type Signal } from "./signal";
import { FlowTrace } from "./flow";
import { useVisible } from "../motion/useVisible";

/** read returns an existing snapshot. Rendering never collects or invents samples. */
export function Scope({ label, unit, samples, read, active = false, signal = "edit", window = 120, scale = "rate", flow = false, identity = null }: {
  label: string; unit: string; samples: Series; read?: () => Series; active?: boolean; signal?: Signal; window?: 64 | 120; scale?: "rate" | "extent"; flow?: boolean; identity?: string | number | null;
}) {
  const host = useRef<HTMLDivElement>(null), line = useRef<SVGPathElement>(null), fill = useRef<SVGPathElement>(null);
  const value = useRef<HTMLSpanElement>(null), status = useRef<HTMLSpanElement>(null), hold = useRef<SVGPathElement>(null);
  const reduced = useReducedMotion(), visible = useVisible(host);
  const trail = useRef(new FlowTrace()), owner = useRef(identity), maximum = useRef(1);
  const snapshot = () => recent(read?.() ?? samples, window);
  const draw = (data: (number | null)[]) => {
    const changedOwner = owner.current !== identity;
    if (changedOwner) { trail.current.reset(data); owner.current = identity; } else trail.current.update(data);
    if (!flow || data.some(value => value !== null) || changedOwner) maximum.current = ceiling(flow && scale === "extent" ? trail.current.samples() : data, scale === "extent" ? "context" : "rate");
    const paths = flow ? { line: trail.current.path(maximum.current), fill: "" } : plot(data, maximum.current);
    if (line.current?.getAttribute("d") !== paths.line) line.current?.setAttribute("d", paths.line);
    if (fill.current?.getAttribute("d") !== paths.fill) fill.current?.setAttribute("d", paths.fill);
    hold.current?.setAttribute("d", flow ? trail.current.holdPath(maximum.current) : "");
  };
  const numbers = (data: (number | null)[]) => {
    if (value.current) value.current.textContent = describeSeries(data, unit);
    if (status.current) status.current.textContent = data.length === 0 ? "No samples" : data.every(v => v === null) ? "History unavailable"
      : data.some(v => v === null) ? "Partial history" : data.every(v => v === 0) ? "Idle" : `${data.length} samples`;
  };
  useLayoutEffect(() => { const data = snapshot(); draw(data); numbers(data); }, [samples, window, unit, active, reduced, scale, flow, identity]);
  useTicker(host, (_now, dt) => {
    if (flow) {
      if (active && read) draw(snapshot());
      trail.current.advance(dt);
      line.current?.setAttribute("d", trail.current.path(maximum.current));
      hold.current?.setAttribute("d", trail.current.holdPath(maximum.current));
      host.current?.style.setProperty("background-position-x", `${-trail.current.phase}px`);
    } else draw(snapshot());
  }, 30, visible && active && (flow || !!read));
  useReadout(host, () => {
    const data = snapshot(); numbers(data);
    if (reduced) draw(data);
  }, visible && active && !!read);
  return <Lcd ref={host} grid className="synth-graph" data-flow={flow || undefined} data-active={active} style={signalStyle(signal)}
    title={flow ? "Scrolling reported values, not a time axis. A dashed tail holds the last reported value when the latest reading is unavailable; it is not a new measurement. Unknown history stays unknown." : undefined}>
    <div className="synth-graph__heading"><span className="synth-silk">{label}</span><span ref={value}>{describeSeries(recent(samples, window), unit)}</span></div>
    <svg viewBox="0 0 300 80" preserveAspectRatio="none" aria-hidden="true">
      <path className="synth-graph__baseline" d="M0 78H300" />
      <path ref={fill} className="synth-graph__fill" /><path ref={line} className="synth-graph__line" vectorEffect="non-scaling-stroke" />
      {flow && <path ref={hold} className="synth-graph__hold" vectorEffect="non-scaling-stroke" />}
    </svg>
    <span ref={status} className={flow ? "synth-sr-only" : "synth-muted"} />
    {flow && <span className="synth-sr-only">A dashed tail is the last reported value, not a current measurement.</span>}
  </Lcd>;
}
