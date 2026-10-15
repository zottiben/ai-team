import { useMemo, useRef } from "react";
import type { RunEvent } from "../../../api";
import { useMotionVisible } from "../../motion/useMotionVisible";
import { Lcd } from "../../primitives/Lcd";
import { signalStyle } from "../../primitives/signal";
import { arrangementScale, clipActivity, durationLabel, durationMs, firstLine, runSignal, type RunRecord } from "./runs";

const LEGEND = [
  { kind: "done", label: "Done" },
  { kind: "cancelled", label: "Cancelled" },
  { kind: "live", label: "Live" },
  { kind: "other", label: "Other" },
] as const;

/** A wave drawn from recorded events only; an empty clip stays an empty clip. */
function wavePath(values: readonly number[]): string {
  const last = Math.max(1, values.length - 1);
  return values.map((value, index) => `${index ? "L" : "M"}${((index / last) * 100).toFixed(1)} ${(44 - value * 32).toFixed(1)}`).join(" ");
}

/**
 * The clip lane (§9.1), measured in wall duration.
 *
 * One clip per run, sized by the time between its start and its end. When any run cannot
 * be measured the lane says so and falls back to equal widths with no ruler, because a
 * proportional lane with a guessed clip in it is a lie about every other clip.
 */
export function Arrangement({ records, events, now, selected, onSelect }: {
  records: readonly RunRecord[]; events: readonly RunEvent[]; now: number;
  selected: number | null; onSelect: (runId: number) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const motion = useMotionVisible(host);
  const byNode = useMemo(() => {
    const result = new Map<number, RunEvent[]>();
    for (const event of events) {
      if (event.node_run_id === null) continue;
      const rows = result.get(event.node_run_id) ?? [];
      rows.push(event);
      result.set(event.node_run_id, rows);
    }
    return result;
  }, [events]);
  const durations = records.map(record => durationMs(record, now));
  const scale = arrangementScale(durations);
  return <Lcd grid ref={host} className="synth-arrangement" role="group" aria-label="Arrangement">
    <div className="synth-arrangement__heading">
      <span className="synth-silk">Arrangement · wall duration, start to end</span>
      <span className="synth-arrangement__legend">
        {LEGEND.map(entry => <span key={entry.kind} className="synth-silk" data-kind={entry.kind}>{entry.label}</span>)}
      </span>
    </div>
    {scale.proportional
      ? <div className="synth-arrangement__ruler" aria-hidden="true">
        {scale.ticks.map(tick => <span key={tick.ms} className="synth-mono" style={{ left: `${(tick.fraction * 100).toFixed(2)}%` }}>{tick.label}</span>)}
      </div>
      : <p className="synth-muted">{durations.some(value => value === null)
        ? "No ruler: at least one run has no valid recorded start and end, so every clip takes an equal width."
        : "No ruler: recorded durations total zero; clips use equal widths."}</p>}
    <div className="synth-arrangement__lane" data-motion={motion} data-proportional={scale.proportional}>
      {records.map((record, index) => {
        const ms = durations[index] ?? null;
        const wave = clipActivity(byNode.get(record.nodeId) ?? [], record, now);
        const detail = `Run #${record.runId} · ${record.label} · ${durationLabel(ms)} · ${firstLine(record.prompt)}`;
        return <button key={record.runId} type="button" className="synth-clip" data-kind={record.kind} data-status={record.status}
          data-live={record.live || undefined} aria-current={selected === record.runId ? "true" : undefined}
          style={{ ...signalStyle(runSignal(record)), flexGrow: scale.proportional ? (ms ?? 0) / 60_000 : 1 }}
          title={detail} aria-label={detail} onClick={() => onSelect(record.runId)}>
          {wave && <svg className="synth-clip__wave" viewBox="0 0 100 52" preserveAspectRatio="none" aria-hidden="true">
            <path d={wavePath(wave)} fill="none" stroke="currentColor" strokeWidth="1.2" vectorEffect="non-scaling-stroke" />
          </svg>}
          <span className="synth-clip__number synth-mono" aria-hidden="true">{record.runId}</span>
          {record.live && <span className="synth-clip__playhead" aria-hidden="true" />}
        </button>;
      })}
      {!records.length && <p className="synth-muted">No runs yet.</p>}
    </div>
  </Lcd>;
}
