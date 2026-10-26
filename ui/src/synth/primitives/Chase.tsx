import { useLayoutEffect, useRef } from "react";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";

export function Chase({ working }: { working: boolean }) {
  const host = useRef<HTMLSpanElement>(null);
  const position = useRef(0);
  const reduced = useReducedMotion();
  const draw = () => host.current?.querySelectorAll("i").forEach((bar, i) => {
    const distance = (position.current - i + 24) % 24;
    bar.dataset.lit = String(working && distance < 7);
    bar.style.opacity = working && distance < 7 ? String(Math.max(0, 1 - distance * 0.22)) : "1";
  });
  useLayoutEffect(() => { position.current = 12; draw(); }, [working, reduced]);
  useTicker(host, () => { position.current = (position.current + 1) % 24; draw(); }, 10, working);
  return <span ref={host} className="synth-segments synth-chase" aria-hidden="true">{Array.from({ length: 24 }, (_, i) => <i key={i} />)}</span>;
}
