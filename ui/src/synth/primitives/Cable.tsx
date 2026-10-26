import { useLayoutEffect, useRef } from "react";
import { cablePath, spring, type Point } from "../motion/spring";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useTicker } from "../motion/useTicker";
import { signalStyle, type Signal } from "./signal";

/** Visual only. Gate/patch authorization and keyboard-operable plugs belong to the host. */
export function Cable({ from, to, signal = "diff", connected, eventId }: {
  from: Point; to: Point; signal?: Signal; connected: boolean; eventId?: string | number;
}) {
  const group = useRef<SVGGElement>(null);
  const elapsed = useRef(0);
  const reduced = useReducedMotion();
  const draw = (factor: number) => group.current?.querySelectorAll("path").forEach(path => path.setAttribute("d", cablePath(from, to, factor)));
  const identity = `${connected}:${eventId ?? ""}`;
  useLayoutEffect(() => {
    elapsed.current = reduced || eventId === undefined || !connected ? 1200 : 0;
    draw(spring(elapsed.current / 1000));
  }, [identity]);
  useLayoutEffect(() => {
    if (reduced) elapsed.current = 1200;
    draw(spring(elapsed.current / 1000));
  }, [from.x, from.y, to.x, to.y, reduced]);
  useTicker(group, (_now, dt) => {
    elapsed.current += dt;
    draw(spring(elapsed.current / 1000));
    return elapsed.current < 1200;
  }, 30, connected && eventId !== undefined, identity);
  const d = cablePath(from, to);
  return <g ref={group} className="synth-cable" data-connected={connected} style={signalStyle(signal)} aria-hidden="true">
    <path className="synth-cable__shadow" d={d} />
    <path className="synth-cable__body" d={d} />
    <path className="synth-cable__shine" d={d} />
    {[from, to].map((p, i) => <g key={i}><circle className="synth-cable__plug" cx={p.x} cy={p.y} r={8.5} /><circle className="synth-cable__hole" cx={p.x} cy={p.y} r={3.2} /></g>)}
  </g>;
}
