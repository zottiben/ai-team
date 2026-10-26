import { useMemo, useState } from "react";
import type { RunEvent } from "../../../api";
import type { ChatDetail } from "../../../chat-api";
import type { ChatMapView, ChatOverview } from "../../../chat-overview-api";
import { compact } from "../../../chat-overview";
import { completedRates, observedRates, type Measurement } from "../../data/telemetry";
import { toolDurations } from "../../data/tool-trace";
import { Chase } from "../../primitives/Chase";
import { HwButton } from "../../primitives/HwButton";
import { Knob } from "../../primitives/Knob";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { Plate } from "../../primitives/Plate";
import { Scope } from "../../primitives/Scope";
import TokenScope from "./TokenScope";
import "./overview.css";

export default function OverviewRack({ detail, status, events, history, live, view, elapsed, now }: {
  detail: ChatDetail; status: string; events: RunEvent[]; history: readonly Measurement[]; live: ChatOverview | null; view: ChatMapView | null; elapsed: number; now: number;
}) {
  const [rateView, setRateView] = useState<"steps" | "receive">("steps");
  const running = detail.state === "running" || detail.state === "stopping";
  const node = detail.active_node_id ?? detail.turns.at(-1)?.node.id ?? null;
  const observed = useMemo(() => observedRates(history), [history]);
  const durations = useMemo(() => node === null ? [] : toolDurations(events, node), [events, node]);
  const lastStamp = history.at(-1)?.observation?.at;
  const end = running ? now : lastStamp ? Date.parse(lastStamp) : now;
  const completed = useMemo(() => completedRates(history, end), [history, end]);
  const latest = events.filter(event => !event.telemetry_only && event.node_run_id === node).at(-1);
  const files = view?.map.files ?? null;
  const covered = files && view ? (files - view.map.unowned) / files : null;
  const edited = live && live.totals.files_touched > 0 ? live.totals.files_written / live.totals.files_touched : null;
  const usage = live?.totals.usage;
  return <div className="synth-overview-rack">
    <TokenScope history={history} node={node} running={running} />
    <Plate heading="Execution" tone="night" aria-label="Synth execution" actions={<div className="synth-execution-state"><Led state={running ? "pulse" : "on"} signal={running ? "working" : detail.state === "failed" || detail.state.includes("interrupted") ? "attention" : "idle"} bezel /><strong>{status}</strong></div>}>
      <div className="synth-execution-main"><Lcd><span className="synth-silk">Wall elapsed</span><strong>{running && Number.isFinite(elapsed) ? `${elapsed}s` : "—"}</strong></Lcd>
        <div><strong>{detail.mode === "team" ? "Pi team" : "Single Pi agent"}</strong><p className="synth-workspace-name mono" title={detail.workspace_path}>{detail.workspace_path}</p><p className="synth-muted">Chat #{detail.id} · {node === null ? "no recorded node" : `node #${node}`}</p></div></div>
      <div className="synth-rate-selector" role="group" aria-label="Output scope source"><HwButton latched={rateView === "steps"} onClick={() => setRateView("steps")}>Step buckets</HwButton><HwButton latched={rateView === "receive"} onClick={() => setRateView("receive")}>Receive intervals</HwButton></div>
      <div className="synth-execution-plot" hidden={rateView !== "steps"}><Scope label="Completed step output" unit="tok/s" samples={completed} window={64} signal="read" flow active={running} identity={node} /></div>
      <div className="synth-execution-plot" hidden={rateView !== "receive"}><Scope label="Observed output change" unit="tok/s" samples={observed} signal="working" flow active={running} identity={node} /></div>
      <details className="synth-rate-provenance"><summary aria-label="Rate provenance and limits" title="Rate provenance and limits">ⓘ</summary>
        <p className="synth-muted">Completed steps: 15s buckets over a 16-minute recorded window. Last bucket is partial; gaps are unavailable, not zero. This is not instantaneous generation speed.</p>
        <p className="synth-muted">Receive intervals: cumulative counter differences over same-message host receive intervals, not execution time. Latest 120 samples; new process/session gaps remain visible.</p>
      </details>
      <div role="group" aria-label="Tool runtime"><Scope label="Tool runtime" unit="ms" samples={durations} signal="bash" scale="extent" flow active={running} identity={node}/></div>
      <details className="synth-rate-provenance"><summary aria-label="Tool runtime provenance" title="Tool runtime provenance">ⓘ</summary><p className="synth-muted">Latest 120 completed tool durations explicitly reported by the runtime, in result receipt order. No receive-time estimates or pending-tool clock. Missing, ambiguous and different-reader evidence breaks the line.</p></details>
      <Chase working={running} />
      <div className="synth-execution-minis"><Lcd><span className="synth-silk">Activity</span><span title={latest?.summary}>{latest?.summary ?? "No recorded activity"}</span></Lcd>
        <Lcd><span className="synth-silk">Last event</span><time dateTime={latest?.at} title={latest?.at}>{latest?.at ?? "Unavailable"}</time></Lcd>
        <Lcd><span className="synth-silk">Turns</span><strong>{detail.turns.length}</strong></Lcd></div>
    </Plate>
    <Plate heading="Meters" aria-label="Chat meters" actions={live && live.nodes_read < live.nodes_total ? <span className="synth-muted">Newest {live.nodes_read}/{live.nodes_total} agent turns</span> : null}>
      <div className="synth-overview-meters">
        <Lcd><span className="synth-silk">Files</span><strong title={`${files ?? "Unknown"} walked files`}>{files === null ? "N/A" : compact(files)}</strong></Lcd>
        <Knob label="Covered" readOnly size={40} signal="read" value={covered ?? 0} valueText={() => covered === null ? "N/A" : `${Math.round(covered * 100)}%`} />
        <Lcd><span className="synth-silk">Turns</span><strong>{live?.totals.turns ?? "N/A"}</strong></Lcd>
        <Lcd className="synth-meter-tokens"><span className="synth-silk">Tokens</span><strong>{usage ? compact(usage.tokens_in + usage.tokens_out) : "N/A"}</strong></Lcd>
        <Knob label="Edited" readOnly size={40} signal="bash" value={edited ?? 0} valueText={() => edited === null ? "N/A" : `${Math.round(edited * 100)}%`} />
      </div>
      <details className="synth-rate-provenance"><summary aria-label="File ratios and token sources" title="File ratios and token sources">ⓘ</summary>
      <p className="synth-muted">Covered = zone-covered walked files. Edited = {live ? `${live.totals.files_written}/${live.totals.files_touched}` : "unknown"} written/touched paths, not task progress.</p>
      <Lcd className="synth-usage-breakdown"><span>Uncached input: {usage?.tokens_in ?? "N/A"}</span><span>Output: {usage?.tokens_out ?? "N/A"}</span><span>Cache read: {usage?.cache_read ?? "N/A"}</span><span>Cache write: {usage?.cache_write ?? "N/A"}</span><span>Model steps: {live?.totals.steps ?? "N/A"}</span></Lcd>
      <p className="synth-muted">Token total excludes caches. Telemetry uses at most the newest 4,096 receipts for this node; all older evidence remains stored.</p>
      </details>
    </Plate>
  </div>;
}
