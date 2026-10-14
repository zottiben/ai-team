import { useRef } from "react";
import { useAttention } from "../SynthProvider";
import { useMotionVisible } from "../motion/useMotionVisible";
import { signalStyle, type Signal } from "./signal";

export type LedState = "off" | "on" | "pulse" | "event" | "attention";

/** Decorative: the caller supplies an adjacent text label. One attention LED per view. */
export function Led({ state, signal = "done", size = 8, bezel = false, eventId }: {
  state: LedState; signal?: Signal; size?: 6 | 7 | 8 | 10 | 12; bezel?: boolean;
  eventId?: string | number;
}) {
  const host = useRef<HTMLSpanElement>(null);
  const visible = useMotionVisible(host);
  const attention = useAttention(state === "attention" && visible);
  const motion = visible && (state !== "attention" || attention);
  return <span ref={host} className="synth-led-host" data-bezel={bezel || undefined} aria-hidden="true">
    <span key={state === "event" ? eventId : state} className="synth-led" data-state={state} data-motion={motion}
      style={{ ...signalStyle(state === "attention" ? "attention" : signal), width: size, height: size }} />
  </span>;
}
