import { useRef } from "react";
import { Led } from "../../../primitives/Led";
import { useMotionVisible } from "../../../motion/useMotionVisible";
import { instant } from "../../Work/runs";

export default function LiveRow({ stopping, started, now }: { stopping: boolean; started: string | null | undefined; now: number }) {
  const host = useRef<HTMLDivElement>(null);
  const motion = useMotionVisible(host);
  const start = instant(started);
  const elapsed = start !== null && Number.isFinite(now) && now >= start ? Math.floor((now - start) / 1000) : null;
  return <div ref={host} className="chat-working synth-tracker-live" data-motion={motion && !stopping}>
    <Led state={stopping ? "on" : "pulse"} signal="live"/>
    <span role="status">{stopping ? "Stopping the current turn…" : "Working…"}</span>
    <span className="synth-tracker-live-indicator" aria-hidden="true" title="Activity indicator, not measured throughput"><span/></span>
    <span className="synth-tracker-meta">{elapsed === null ? "Wall elapsed unavailable" : `${elapsed}s wall elapsed`}</span>
  </div>;
}
