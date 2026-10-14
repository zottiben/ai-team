import { useRef } from "react";
import { Led } from "../../../primitives/Led";
import { useMotionVisible } from "../../../motion/useMotionVisible";

export default function LiveRow({ stopping, elapsed }: { stopping: boolean; elapsed: number }) {
  const host = useRef<HTMLDivElement>(null);
  const motion = useMotionVisible(host);
  return <div ref={host} className="chat-working synth-tracker-live" role="status" data-motion={motion && !stopping}>
    <Led state={stopping ? "on" : "pulse"} signal="live"/>
    <span>{stopping ? "Stopping the current turn…" : "Working…"}</span>
    <span className="synth-tracker-live-indicator" aria-hidden="true" title="Activity indicator, not measured throughput"><span/></span>
    <span className="synth-tracker-meta">{Number.isFinite(elapsed) ? elapsed : 0}s wall elapsed</span>
  </div>;
}
