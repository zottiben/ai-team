import { useRef } from "react";

/** §11.2: "Width ∝ line length (cap 27)". */
const CAP = 27;
/** Past this, one bar per line is more bars than pixels - so lines are bucketed. */
const BUCKET_ABOVE = 600;
const WIDTH = 76;

export type Bar = { length: number; added: boolean; removed: boolean };

/**
 * One bar per line, or per bucket of lines once a file is long enough that a bar would
 * be less than a pixel tall. A bucket takes the longest line in it and any marker any of
 * its lines carries, so a single changed line in a long file is still visible.
 */
export function bars(lengths: number[], added: Set<number>, removed: Set<number>): Bar[] {
  const count = lengths.length;
  if (count === 0) return [];
  const size = count > BUCKET_ABOVE ? Math.ceil(count / BUCKET_ABOVE) : 1;
  const out: Bar[] = [];
  for (let start = 0; start < count; start += size) {
    let length = 0;
    let isAdded = false;
    let isRemoved = false;
    for (let index = start; index < Math.min(start + size, count); index += 1) {
      length = Math.max(length, lengths[index] ?? 0);
      isAdded ||= added.has(index + 1);
      isRemoved ||= removed.has(index + 1);
    }
    out.push({ length: Math.min(length, CAP), added: isAdded, removed: isRemoved });
  }
  return out;
}

/**
 * The waveform minimap.
 *
 * Drawn from the real buffer: one bar per line mirrored around the centre, with the
 * change markers in their own colours and a window over the lines the editor is actually
 * showing. Dragging the window scrolls; clicking jumps.
 */
export default function WaveMinimap({ lengths, added, removed, viewport, onJump }: {
  lengths: number[];
  added: Set<number>;
  removed: Set<number>;
  /** One-based, inclusive. Null before the editor has reported one. */
  viewport: { from: number; to: number } | null;
  onJump: (line: number) => void;
}) {
  const host = useRef<HTMLDivElement>(null), plot = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const rows = bars(lengths, added, removed);
  const total = Math.max(lengths.length, 1);

  const lineAt = (clientY: number): number => {
    const box = plot.current?.getBoundingClientRect();
    if (box === undefined || box.height === 0) return 1;
    const fraction = Math.min(1, Math.max(0, (clientY - box.top) / box.height));
    return Math.min(total, Math.max(1, Math.round(fraction * total)));
  };

  const window = viewport === null ? null : {
    top: ((viewport.from - 1) / total) * 100,
    height: Math.max(2, ((viewport.to - viewport.from + 1) / total) * 100),
  };

  return <div
    ref={host}
    className="synth-minimap"
    role="scrollbar"
    aria-label="File map"
    aria-orientation="vertical"
    aria-controls="synth-editor-code"
    aria-valuenow={viewport?.from ?? 1}
    aria-valuemin={1}
    aria-valuemax={total}
    tabIndex={0}
    onPointerDown={event => {
      dragging.current = true;
      host.current?.setPointerCapture(event.pointerId);
      onJump(lineAt(event.clientY));
    }}
    onPointerMove={event => { if (dragging.current) onJump(lineAt(event.clientY)); }}
    onPointerUp={event => {
      dragging.current = false;
      if (host.current?.hasPointerCapture(event.pointerId)) {
        host.current.releasePointerCapture(event.pointerId);
      }
    }}
    onKeyDown={event => {
      const at = viewport?.from ?? 1;
      const step = Math.max(1, Math.round(total / 20));
      if (event.key === "ArrowDown") { event.preventDefault(); onJump(Math.min(total, at + step)); }
      if (event.key === "ArrowUp") { event.preventDefault(); onJump(Math.max(1, at - step)); }
      if (event.key === "Home") { event.preventDefault(); onJump(1); }
      if (event.key === "End") { event.preventDefault(); onJump(total); }
    }}
  >
    <div ref={plot} className="synth-minimap__plot" style={{ height: Math.max(rows.length, 1) * 4 }}>
    <svg
      className="synth-minimap__wave"
      viewBox={`0 0 ${WIDTH} ${Math.max(rows.length, 1)}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      {rows.map((bar, index) => {
        const width = (bar.length / CAP) * (WIDTH - 8);
        return <rect
          key={index}
          x={(WIDTH - width) / 2}
          y={index}
          width={Math.max(width, bar.length === 0 ? 0 : 1)}
          height={0.8}
          className="synth-minimap__bar"
          data-kind={bar.removed ? "removed" : bar.added ? "added" : "line"}
        />;
      })}
    </svg>
    {window !== null && <div
      className="synth-minimap__window"
      style={{ top: `${window.top}%`, height: `${window.height}%` }}
      aria-hidden="true"
    />}
    </div>
  </div>;
}
