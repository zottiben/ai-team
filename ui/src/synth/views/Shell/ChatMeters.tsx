import { useMemo, useRef, useState } from "react";
import { useChatDisplay } from "../../data/chat-display";
import { chatMeters } from "../../data/chat-meters";
import { useReadoutSnapshot } from "../../motion/useReadoutSnapshot";
import { useReadout } from "../../motion/readouts";
import { VuMeter } from "../../primitives/VuMeter";
import { ceiling } from "../../primitives/graph";

export function ChatMeters({ chat, project }: { chat: number | null; project: number | null }) {
  const host = useRef<HTMLDivElement>(null), current = useChatDisplay(chat, project);
  const scope = JSON.stringify([chat, project, current?.detail.workspace_path, current?.detail.workspace_epoch, current?.detail.mode]);
  const value = useReadoutSnapshot(host, current, scope, true), [now, setNow] = useState(Date.now);
  useReadout(host, () => setNow(Date.now()), value?.detail.state === "running" || value?.detail.state === "stopping");
  const metrics = useMemo(() => value ? chatMeters(value, now) : null, [value, now]);
  return <div ref={host} className="synth-title-metrics" aria-label="Selected chat telemetry" hidden={!metrics}>
    {metrics && <>
      <div><span className="synth-silk">Chat #{chat} · captured</span><span className="mono">Tokens {metrics.tokens ?? "N/A"} · Prompt {metrics.context ?? "N/A"}</span></div>
      <div className="synth-title-vu"><VuMeter label="Input" value={metrics.input.at(-1) == null ? NaN : metrics.input.at(-1)! / ceiling(metrics.input, "rate")}/><VuMeter label="Output" value={metrics.output.at(-1) == null ? NaN : metrics.output.at(-1)! / ceiling(metrics.output, "rate")}/></div>
      <details><summary aria-label="Selected chat metric provenance">?</summary><p>Only chat #{chat}. Uncached input + output from {metrics.steps} captured completed steps; not all historical spend. Caches excluded. Prompt is the latest reported context of node #{metrics.node ?? "unknown"}, not a sum or model capacity. Meters are completed input/output per 15s wall bucket, each scaled to its 16-minute p95 × 1.2; not instantaneous generation. Missing buckets are unknown. At most 4,096 captured receipts per node; no historical backfill.</p></details>
    </>}
  </div>;
}
