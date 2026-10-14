import { ratio } from "../motion/ballistics";
import { signalStyle, type Signal } from "./signal";

export function SegmentMeter({ label, value, segments = 6, signal = "edit" }: {
  label: string; value: number; segments?: 6 | 8 | 12 | 16; signal?: Signal;
}) {
  const lit = Math.round(ratio(value / segments) * segments);
  return <div className="synth-meter" style={signalStyle(signal)}>
    <span>{label} · {Number.isFinite(value) ? value : "unavailable"}</span>
    <span className="synth-segments" aria-hidden="true">{Array.from({ length: segments }, (_, i) => <i key={i} data-lit={i < lit} />)}</span>
  </div>;
}
