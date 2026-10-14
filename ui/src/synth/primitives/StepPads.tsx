import { Fragment, useEffect, useRef, useState } from "react";
import { useMotionVisible } from "../motion/useMotionVisible";
import { observation } from "./graph";
import { signalStyle, type Signal } from "./signal";

export interface PatternStep { id: string; lane: string; label: string; durationMs?: number | null; current?: boolean }
export function StepPads({ label, lanes, steps, limit = 48, arriving, onSelect }: {
  label: string; lanes: readonly { id: string; label: string; signal: Signal }[]; steps: readonly PatternStep[];
  limit?: 48 | 96 | "all"; arriving?: string; onSelect?: (id: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null), motion = useMotionVisible(host);
  const [fresh, setFresh] = useState(arriving);
  useEffect(() => {
    setFresh(arriving);
    if (!arriving) return;
    const timer = setTimeout(() => setFresh(undefined), 160);
    return () => clearTimeout(timer);
  }, [arriving]);
  const offset = limit === "all" ? 0 : Math.max(0, steps.length - limit);
  const visible = steps.slice(offset);
  const longest = visible.reduce((max, step) => Math.max(max, observation(step.durationMs) ?? 0), 0);
  const unknown = visible.filter(step => !lanes.some(lane => lane.id === step.lane));
  return <div ref={host} className="synth-pattern" role="group" aria-label={label} data-motion={motion}>
    <div className="synth-graph__heading"><span className="synth-silk">{label}</span><span>{visible.length} of {steps.length} steps</span></div>
    <div className="synth-pattern__scroll"><div className="synth-pattern__grid" style={{ gridTemplateColumns: `70px repeat(${Math.max(1, visible.length)}, minmax(32px, 1fr))` }}>
      <span aria-hidden="true" />{visible.length ? visible.map((s, i) => <span key={s.id} className="synth-pattern__ruler" aria-hidden="true">{(offset + i + 1) % 8 === 0 ? offset + i + 1 : ""}</span>) : <span />}
      {lanes.map(lane => <Fragment key={lane.id}>
        <span className="synth-pattern__lane">{lane.label}</span>
        {visible.length ? visible.map(step => {
          if (step.lane !== lane.id) return <span key={step.id} className="synth-step-empty" aria-hidden="true" />;
          const duration = observation(step.durationMs);
          const intensity = duration === null ? 1 : .55 + .45 * (longest ? Math.log1p(duration) / Math.log1p(longest) : 0);
          const title = `${lane.label} · ${step.label} · ${duration === null ? "duration unavailable" : `${duration}ms`}`;
          return <button key={step.id} type="button" className="synth-step" aria-label={title} title={title} aria-current={step.current ? "step" : undefined}
            data-arriving={step.id === fresh || undefined} disabled={!onSelect} style={signalStyle(lane.signal)} onClick={() => onSelect?.(step.id)}>
            <span style={{ opacity: intensity }} aria-hidden="true" />
          </button>;
        }) : <span className="synth-step-empty" aria-hidden="true" />}
      </Fragment>)}
    </div></div>
    {!steps.length && <span className="synth-muted">No steps</span>}
    {unknown.length > 0 && <p className="synth-muted">Unmapped steps: {unknown.map(s => s.label).join(", ")}</p>}
  </div>;
}
