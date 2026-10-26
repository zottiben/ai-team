import { useLayoutEffect, useRef, useState } from "react";
import { Cable } from "../../primitives/Cable";
import { patchPoint } from "../../primitives/patch";
import { useVisible } from "../../motion/useVisible";
import { SeatModule } from "./SeatModule";
import type { PatchEvidence } from "./evidence";

type Point = { x: number; y: number };
type Layout = { width: number; height: number; ports: Map<string, Point> };
export function PatchCanvas({ graph, selected, onSelect, zoom, compact = false }: { graph: PatchEvidence; selected: string | null; onSelect: (id: string) => void; zoom: number; compact?: boolean }) {
  const host = useRef<HTMLDivElement>(null), visible = useVisible(host);
  const [layout, setLayout] = useState<Layout | null>(null);
  useLayoutEffect(() => {
    const element = host.current;
    if (!element || !visible) return;
    const measure = () => {
      if (!element.getClientRects().length) return;
      const bounds = element.getBoundingClientRect(), ports: Layout["ports"] = new Map();
      const width = element.offsetWidth, height = element.offsetHeight;
      if (!width || !height || !bounds.width || !bounds.height) return;
      for (const node of element.querySelectorAll<HTMLElement>("[data-patch-id]")) {
        const rect = node.querySelector(".synth-jack__socket")?.getBoundingClientRect(), id = node.dataset.patchId;
        if (id && rect?.width && rect.height) ports.set(id, patchPoint({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 }, bounds, width, height));
      }
      setLayout(previous => {
        const next = { width, height, ports };
        return previous && previous.width === next.width && previous.height === next.height && JSON.stringify([...previous.ports]) === JSON.stringify([...ports]) ? previous : next;
      });
    };
    measure(); const observer = new ResizeObserver(measure); observer.observe(element);
    element.querySelectorAll("[data-patch-id], .synth-jack").forEach(node => observer.observe(node));
    return () => observer.disconnect();
  }, [graph, visible, zoom, compact]);
  return <div className="synth-patch-viewport" tabIndex={0} aria-label="Execution evidence canvas · scroll to inspect modules">
    <div className="synth-patch-canvas" ref={host} style={{ minWidth: `${(compact ? 480 : 760) * zoom}px` }}>
      {(["controller", "member", "draft"] as const).map(kind => <div className="synth-patch-column" key={kind} aria-label={`${kind} modules`}>
        {graph.modules.filter(module => module.kind === kind).map(module => <SeatModule key={module.id} module={module} selected={module.id === selected} onSelect={() => onSelect(module.id)}/>)}
      </div>)}
      {visible && layout && <svg className="synth-patch-cables" width="100%" height="100%" viewBox={`0 0 ${layout.width} ${layout.height}`} preserveAspectRatio="none" aria-hidden="true">{graph.links.map(link => {
        const from = layout.ports.get(link.from), to = layout.ports.get(link.to);
        return from && to ? <g key={JSON.stringify([link.from, link.to, link.kind])} data-patch-from={link.from} data-patch-to={link.to}>
          <Cable from={from} to={to} connected signal={link.kind === "slice_assignment" ? "diff" : "plan"}/>
        </g> : null;
      })}</svg>}
    </div>
  </div>;
}
