import { useMemo } from "react";
import { tokenTrace, type Measurement } from "../../data/telemetry";
import { Plate } from "../../primitives/Plate";
import { Scope } from "../../primitives/Scope";

/** Presentation of the existing exact-node feed. No sampling clock or second fetch. */
export default function TokenScope({ history, node, running }: {
  history: readonly Measurement[]; node: number | null; running: boolean;
}) {
  const trace = useMemo(() => tokenTrace(history, "levels"), [history]);
  return <Plate heading="Token scope" tone="night" className="synth-token-scope" aria-label="Live token scope">
    <span className="synth-sr-only">{node === null ? "No recorded node" : `${running ? "Following" : "Recorded"} node #${node}`}</span>
    <div className="synth-token-scope__channels">
      <div role="group" aria-label="Input tokens"><Scope label="Input tokens" unit="input tokens" samples={trace.input} signal="read" scale="extent" flow active={running} identity={node} /></div>
      <div role="group" aria-label="Output tokens"><Scope label="Output tokens" unit="output tokens" samples={trace.output} signal="working" scale="extent" flow active={running} identity={node} /></div>
      <div role="group" aria-label="Context window"><Scope label="Context window" unit="prompt tokens" samples={trace.context} signal="edit" scale="extent" flow active={running} identity={node} /></div>
      <div role="group" aria-label="Cache read tokens"><Scope label="Cache read tokens" unit="cached tokens" samples={trace.cache_read} signal="read" scale="extent" flow active={running} identity={node} /></div>
      <div role="group" aria-label="Cache write tokens"><Scope label="Cache write tokens" unit="new cache tokens" samples={trace.cache_write} signal="edit" scale="extent" flow active={running} identity={node} /></div>
    </div>
    <details className="synth-rate-provenance"><summary aria-label="Live scope sources and limits" title="Live scope sources and limits">ⓘ</summary>
      <p className="synth-muted">Input excludes caches. Context is the reported prompt, including caches. Model capacity is not reported. Cache reads and writes are response levels, not accumulated totals or inferred savings.</p>
      <p className="synth-muted">Reported response levels, not totals or token rates. Response resets can lower the line. Each trace is independently auto-scaled; height is not context capacity. The display scrolls and holds the last reported reading; it is not a time axis or proof of activity. Up to 120 observations; process/session changes, compaction and missing counts break the line.</p>
      <p className="synth-muted">Readouts update at most once a second. Providers may report usage only when a response completes; missing counts and initial live zeros stay unknown.</p>
      <p className="synth-muted">{trace.at ? <>Latest usage receipt: <time dateTime={trace.at} title={trace.at}>{new Date(trace.at).toLocaleString()}</time></> : "Usage receipt time unavailable."}</p>
    </details>
  </Plate>;
}
