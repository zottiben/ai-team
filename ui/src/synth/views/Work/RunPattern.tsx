import { useRef } from "react";
import { useMotionVisible } from "../../motion/useMotionVisible";
import { Lcd } from "../../primitives/Lcd";
import { signalStyle } from "../../primitives/signal";
import { durationLabel, durationMs, runSignal, type RunRecord } from "./runs";

/** One pad per run, oldest to newest, numbered by the run it opens (§9.1). */
export function RunPattern({ records, now, selected, onSelect }: {
  records: readonly RunRecord[]; now: number; selected: number | null; onSelect: (runId: number) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const motion = useMotionVisible(host);
  const first = records[0], last = records[records.length - 1];
  return <Lcd ref={host} className="synth-run-pattern" role="group" aria-label="Run pattern">
    <div className="synth-run-pattern__heading">
      <span className="synth-silk">Run pattern</span>
      <span className="synth-mono">{first && last ? `#${first.runId} → #${last.runId}` : "No runs yet"}</span>
    </div>
    <div className="synth-run-pattern__pads" data-motion={motion}>
      {records.map(record => <button key={record.runId} type="button" className="synth-run-pad"
        data-kind={record.kind} data-status={record.status} style={signalStyle(runSignal(record))}
        aria-label={`Run #${record.runId}: ${record.label}, ${durationLabel(durationMs(record, now))}`}
        aria-current={selected === record.runId ? "true" : undefined}
        onClick={() => onSelect(record.runId)}>
        <span className="synth-run-pad__light" aria-hidden="true" />
        <span className="synth-mono" aria-hidden="true">{record.runId}</span>
      </button>)}
    </div>
  </Lcd>;
}
