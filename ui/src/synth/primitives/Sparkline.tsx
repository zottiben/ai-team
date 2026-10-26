import { ceiling, describeSeries, plot, recent, type Series } from "./graph";
import { signalStyle, type Signal } from "./signal";

export function Sparkline({ label, unit, samples, signal = "edit" }: { label: string; unit: string; samples: Series; signal?: Signal }) {
  const data = recent(samples, 40);
  return <span className="synth-sparkline" style={signalStyle(signal)}>
    <svg width="80" height="12" viewBox="0 0 80 12" preserveAspectRatio="none" aria-hidden="true">
      <path className="synth-graph__baseline" d="M0 10H80" />
      <path className="synth-graph__line" d={plot(data, ceiling(data, "rate"), 80, 12).line} vectorEffect="non-scaling-stroke" />
    </svg>
    <span>{label} · {describeSeries(data, unit)}</span>
  </span>;
}
