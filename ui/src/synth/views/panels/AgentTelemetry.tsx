import { useMemo } from "react";
import type { Measurement } from "../../data/telemetry";
import { compactionEvidence, contextTrace, count } from "../../data/telemetry";
import { Envelope } from "../../primitives/Envelope";
import { Lcd } from "../../primitives/Lcd";
import { SegmentMeter } from "../../primitives/SegmentMeter";

const correlation = { paired: "matched explicit receipts", start_only: "end not recorded", end_only: "start not recorded", missing_identity: "scope/sequence unavailable", ambiguous: "conflicting, duplicate or reversed receipts" };

export function TouchCounts({ reads, writes }: { reads: number; writes: number }) {
  const r = count(reads), w = count(writes), total = count((r ?? NaN) + (w ?? NaN));
  return <div className="synth-touch-counts" title="Recorded read/write touches, not net changes. The bar caps at eight; counts do not.">
    <span>R {r ?? "?"} · W {w ?? "?"}</span><SegmentMeter label="Read + write counts" value={total ?? NaN} segments={8} signal={writes > 0 ? "edit" : "read"} />
  </div>;
}

export default function AgentTelemetry({ history, node }: { history: readonly Measurement[]; node: number }) {
  const context = useMemo(() => contextTrace(history), [history]);
  const compactions = useMemo(() => compactionEvidence(history).slice(-16), [history]);
  return <div className="synth-agent-telemetry">
    <Envelope label={`Context · node #${node}`} unit="prompt tokens" retained={context.values} compactions={context.markers} markerLabel="Recorded compaction at plot position" emptyMarkerLabel="No compaction positioned in this step window" />
    <Lcd className="synth-usage-breakdown">Latest reported prompt/context: {context.latest ?? "unavailable"}</Lcd>
    <p className="faint">Up to 120 completed model-step snapshots, not elapsed time or cumulative spend. Process/session boundaries and missing counts break the line. No model context limit is assumed.</p>
    {!!compactions.length && <details><summary>{compactions.length} recent explicit compaction receipts</summary><Lcd><ul>{compactions.map(row => {
      const { event } = row, metric = event.telemetry;
      return <li key={event.id} title={event.observation?.at ?? event.at}>Receipt #{event.id} · {metric.phase} · {correlation[row.correlation]} · {metric.reason ?? "reason unavailable"} · before: {metric.tokens_before ?? "unknown"} · estimated after: {metric.estimated_tokens_after ?? "unknown"}</li>;
    })}</ul></Lcd></details>}
    {history.length === 0 && <p className="faint">No captured telemetry for this node. Historical snapshots below are not reconstructed samples.</p>}
  </div>;
}
