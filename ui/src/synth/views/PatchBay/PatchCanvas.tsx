import { useLayoutEffect, useRef, useState } from "react";
import { Cable } from "../../primitives/Cable";
import { useVisible } from "../../motion/useVisible";
import { SeatModule } from "./SeatModule";
import type { PatchEvidence } from "./evidence";

type Point = { x: number; y: number };
type Layout = { width: number; height: number; ports: Map<string, { from: Point; to: Point }> };
export function PatchCanvas({ graph, selected, onSelect, zoom }: { graph: PatchEvidence; selected: string | null; onSelect: (id: string) => void; zoom: number }) {
  const host = useRef<HTMLDivElement>(null), visible = useVisible(host);
  const [layout, setLayout] = useState<Layout | null>(null);
  useLayoutEffect(() => {
    const element = host.current;
    if (!element || !visible) return;
    const measure = () => {
      if (!element.getClientRects().length) return;
      const bounds = element.getBoundingClientRect(), ports: Layout["ports"] = new Map();
      for (const node of element.querySelectorAll<HTMLElement>("[data-patch-id]")) {
        const rect = node.getBoundingClientRect(), id = node.dataset.patchId;
        if (id && rect.width && rect.height) ports.set(id, { from: { x: rect.right - bounds.left, y: rect.top - bounds.top + rect.height / 2 }, to: { x: rect.left - bounds.left, y: rect.top - bounds.top + rect.height / 2 } });
      }
      setLayout(previous => {
        const next = { width: bounds.width, height: bounds.height, ports };
        return previous && previous.width === next.width && previous.height === next.height && JSON.stringify([...previous.ports]) === JSON.stringify([...ports]) ? previous : next;
      });
    };
    measure(); const observer = new ResizeObserver(measure); observer.observe(element);
    element.querySelectorAll("[data-patch-id]").forEach(node => observer.observe(node));
    return () => observer.disconnect();
  }, [graph, visible, zoom]);
  return <div className="synth-patch-viewport" tabIndex={0} aria-label="Execution evidence canvas · scroll to inspect modules">
    <div className="synth-patch-canvas" ref={host} style={{ minWidth: `${760 * zoom}px` }}>
      {visible && layout && <svg className="synth-patch-cables" width={layout.width} height={layout.height} aria-hidden="true">{graph.links.map(link => {
        const from = layout.ports.get(link.from)?.from, to = layout.ports.get(link.to)?.to;
        return from && to ? <Cable key={JSON.stringify([link.from, link.to, link.kind])} from={from} to={to} connected signal={link.kind === "slice_assignment" ? "diff" : "plan"}/> : null;
      })}</svg>}
      {(["controller", "member", "draft"] as const).map(kind => <div className="synth-patch-column" key={kind} aria-label={`${kind} modules`}>
        {graph.modules.filter(module => module.kind === kind).map(module => <SeatModule key={module.id} module={module} selected={module.id === selected} onSelect={() => onSelect(module.id)}/>)}
      </div>)}
    </div>
  </div>;
}
