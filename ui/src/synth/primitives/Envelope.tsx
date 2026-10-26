import { ceiling, describeSeries, observation, plot, type Series } from "./graph";
import { Lcd } from "./Lcd";
import { signalStyle } from "./signal";

export function Envelope({ label, retained, compactions = [], unit = "retained tokens", markerLabel = "Recorded compaction at turn", emptyMarkerLabel = "No recorded compaction markers" }: { label: string; retained: Series; compactions?: readonly number[]; unit?: string; markerLabel?: string; emptyMarkerLabel?: string }) {
  const paths = plot(retained, ceiling(retained, "context"));
  const markers = [...new Set(compactions)].filter(i => Number.isInteger(i) && i >= 0 && i < retained.length);
  const current = observation(retained[retained.length - 1]);
  return <Lcd grid className="synth-graph" style={signalStyle("edit")}>
    <div className="synth-graph__heading"><span className="synth-silk">{label}</span><span>{describeSeries(retained, unit)}</span></div>
    <svg viewBox="0 0 300 80" preserveAspectRatio="none" aria-hidden="true">
      <path className="synth-graph__baseline" d="M0 78H300" />
      <path className="synth-graph__fill" d={paths.fill} /><path className="synth-graph__line" d={paths.line} vectorEffect="non-scaling-stroke" />
      {retained.length === 1 && current !== null && <circle className="synth-graph__point" cx={150} cy={78 - current / ceiling(retained, "context") * 76} r={2} />}
      {markers.map(index => {
        const x = retained.length <= 1 ? 150 : index / (retained.length - 1) * 300;
        return <path key={index} className="synth-graph__compaction" d={`M${x} 2V78`} vectorEffect="non-scaling-stroke" />;
      })}
    </svg>
    <span className="synth-muted">{markers.length ? `${markerLabel} ${markers.map(i => i + 1).join(", ")}` : emptyMarkerLabel}</span>
  </Lcd>;
}
