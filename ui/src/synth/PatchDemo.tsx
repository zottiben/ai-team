import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useAnnounce } from "./SynthProvider";
import { useReducedMotion } from "./motion/useReducedMotion";
import { useTicker } from "./motion/useTicker";
import { cablePath, type Point } from "./motion/spring";
import { Cable } from "./primitives/Cable";
import { Jack } from "./primitives/Jack";
import { patchPoint } from "./primitives/patch";
import type { Signal } from "./primitives/signal";

const PORTS: readonly { id: string; label: string; output: boolean; signal: Signal }[] = [
  { id: "a", label: "Source A", output: true, signal: "diff" },
  { id: "b", label: "Source B", output: true, signal: "diff" },
  { id: "x", label: "Input X", output: false, signal: "diff" },
  { id: "y", label: "Input Y", output: false, signal: "diff" },
  { id: "z", label: "Verdict only", output: false, signal: "verdict" },
];
type Link = { output: string; event: number };

/** Interaction fixture only. These cables have no runtime or publication authority. */
export function PatchDemo() {
  const host = useRef<HTMLDivElement>(null), ghost = useRef<SVGPathElement>(null);
  const [positions, setPositions] = useState<Record<string, Point>>({});
  const [links, setLinks] = useState<Record<string, Link>>({});
  const [armed, setArmed] = useState<string | null>(null);
  const sequence = useRef(0), pointer = useRef<Point | null>(null);
  const drag = useRef<{ id: number; output: string; start: Point; moved: boolean } | null>(null);
  const skipClick = useRef(false);
  const announce = useAnnounce(), reduced = useReducedMotion();
  const local = (point: Point) => {
    const node = host.current;
    return node ? patchPoint(point, node.getBoundingClientRect(), node.offsetWidth, node.offsetHeight) : point;
  };
  useLayoutEffect(() => {
    const root = host.current;
    if (!root) return;
    const measure = () => {
      const next: Record<string, Point> = {};
      root.querySelectorAll<HTMLButtonElement>("[data-port]").forEach(button => {
        const rect = button.firstElementChild?.getBoundingClientRect();
        if (rect && button.dataset.port) next[button.dataset.port] = local({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
      });
      setPositions(old => Object.keys(next).every(id => old[id]?.x === next[id]?.x && old[id]?.y === next[id]?.y) ? old : next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    root.querySelectorAll("[data-port]").forEach(button => observer.observe(button));
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    return () => { observer.disconnect(); window.removeEventListener("resize", measure); window.removeEventListener("scroll", measure, true); };
  }, []);
  const cancel = () => {
    const captured = drag.current;
    if (captured) skipClick.current = true;
    drag.current = null; setArmed(null); pointer.current = null;
    if (captured && host.current?.hasPointerCapture(captured.id)) host.current.releasePointerCapture(captured.id);
  };
  useEffect(() => {
    if (!armed) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); cancel(); announce("Patch cancelled"); }
    };
    document.addEventListener("keydown", escape);
    return () => document.removeEventListener("keydown", escape);
  }, [armed, announce]);
  const connect = (output: string, input: string) => {
    const from = PORTS.find(p => p.id === output && p.output), to = PORTS.find(p => p.id === input && !p.output);
    if (!from || !to || from.signal !== to.signal) { announce("Incompatible input — no connection changed"); return; }
    const event = ++sequence.current;
    setLinks(old => ({ ...old, [input]: { output, event } }));
    announce(`${from.label} connected to ${to.label}`);
    cancel();
  };
  const activate = (id: string) => {
    const port = PORTS.find(p => p.id === id);
    if (!port) return;
    if (port.output) {
      if (armed === id) { cancel(); announce("Patch cancelled"); }
      else {
        setArmed(id);
        const start = positions[id]; pointer.current = start ? { x: start.x, y: start.y + 30 } : null;
        announce(`${port.label} armed. Choose a compatible input; Escape cancels.`);
      }
    } else if (armed) connect(armed, id);
    else if (links[id]) {
      setLinks(old => { const next = { ...old }; delete next[id]; return next; });
      announce(`${port.label} unplugged`);
    } else announce(`${port.label} is unpatched. Choose a source first.`);
  };
  const drawGhost = () => {
    const output = drag.current?.output ?? armed, from = output ? positions[output] : null;
    if (from && pointer.current) ghost.current?.setAttribute("d", cablePath(from, pointer.current, .6));
  };
  useLayoutEffect(drawGhost, [armed, positions, reduced]);
  useTicker(host, drawGhost, 60, armed !== null);
  return <>
    <p className="synth-muted">Demo only. Click source then input, or drag. Enter/Space activates a focused jack; Escape cancels. Clicking a connected input unplugs it.</p>
    <div ref={host} className="synth-patch" onPointerDown={event => {
      skipClick.current = false;
      const target = event.target instanceof Element ? event.target.closest<HTMLElement>("[data-port]") : null;
      const output = PORTS.find(p => p.id === target?.dataset.port && p.output);
      if (!target) { cancel(); return; }
      if (!output || event.button !== 0 || drag.current) return;
      drag.current = { id: event.pointerId, output: output.id, start: { x: event.clientX, y: event.clientY }, moved: false };
    }} onPointerMove={event => {
      const current = drag.current;
      if (current && current.id !== event.pointerId) return;
      if (current && !current.moved && Math.hypot(event.clientX - current.start.x, event.clientY - current.start.y) >= 4) {
        current.moved = true; setArmed(current.output);
        event.currentTarget.setPointerCapture(event.pointerId);
      }
      if (armed || current?.moved) { pointer.current = local({ x: event.clientX, y: event.clientY }); if (reduced) drawGhost(); }
    }} onPointerUp={event => {
      const current = drag.current;
      if (!current || current.id !== event.pointerId) return;
      drag.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      if (!current.moved) return;
      skipClick.current = true;
      const target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-port]");
      const input = target && host.current?.contains(target) ? target.dataset.port : undefined;
      if (input) connect(current.output, input);
      cancel();
    }} onPointerCancel={event => { if (drag.current?.id === event.pointerId) cancel(); }}
      onLostPointerCapture={event => { if (drag.current?.id === event.pointerId) cancel(); }}>
      <div className="synth-patch__outputs">{PORTS.filter(p => p.output).map(port => <Jack key={port.id} label={port.label} signal={port.signal} armed={armed === port.id} data-port={port.id}
        onClick={event => { if (skipClick.current && event.detail > 0) { skipClick.current = false; return; } activate(port.id); }} />)}</div>
      <div className="synth-patch__inputs">{PORTS.filter(p => !p.output).map(port => {
        const incompatible = !!armed && PORTS.find(p => p.id === armed)?.signal !== port.signal;
        return <Jack key={port.id} label={port.label} signal={port.signal} data-port={port.id} data-compatible={armed ? !incompatible : undefined}
          aria-disabled={incompatible} aria-label={links[port.id] && !armed ? `Unplug ${port.label}` : port.label}
          onClick={() => activate(port.id)} />;
      })}</div>
      <svg className="synth-patch__cables" aria-hidden="true">
        {Object.entries(links).map(([input, link]) => positions[link.output] && positions[input] && <Cable key={input} from={positions[link.output]!} to={positions[input]!} connected eventId={link.event} />)}
        <path ref={ghost} className="synth-patch__ghost" data-armed={armed !== null} />
      </svg>
    </div>
    <p>{Object.keys(links).length} demo connections</p>
    <ul className="synth-muted">{Object.entries(links).map(([input, link]) => <li key={input}>{PORTS.find(p => p.id === link.output)?.label} → {PORTS.find(p => p.id === input)?.label}</li>)}</ul>
  </>;
}
