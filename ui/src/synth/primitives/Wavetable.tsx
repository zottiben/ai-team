import { useEffect, useRef, useState } from "react";
import { useReadout } from "../motion/readouts";
import { useReducedMotion } from "../motion/useReducedMotion";
import { useVisible } from "../motion/useVisible";
import { Lcd } from "./Lcd";
import { observation } from "./graph";
import { signalStyle } from "./signal";

export interface DirectoryActivity { name: string; hits: readonly { position: number; hits: number }[] }
const usable = (hit: DirectoryActivity["hits"][number]) => Number.isFinite(hit.position) && hit.position >= 0 && hit.position <= 1 && observation(hit.hits) !== null;
export function directoryRidges(directories: readonly DirectoryActivity[]) {
  const named = new Map<string, DirectoryActivity["hits"][number][]>();
  for (const directory of directories) named.set(directory.name, [...named.get(directory.name) ?? [], ...directory.hits]);
  const entries = [...named].map(([name, hits]) => ({ names: [name], hits }));
  if (entries.length <= 14) return entries;
  return [...entries.slice(0, 13), { names: entries.slice(13).flatMap(d => d.names), hits: entries.slice(13).flatMap(d => d.hits) }];
}
export function ridgePath(hits: DirectoryActivity["hits"], baseline: number): string {
  return Array.from({ length: 101 }, (_, i) => {
    const position = i / 100;
    const bump = hits.reduce((sum, hit) => {
      if (!usable(hit)) return sum;
      return sum + Math.log1p(hit.hits) * 2.5 * Math.exp(-((position - hit.position) ** 2) / (2 * .04 ** 2));
    }, 0);
    const height = Math.min(24, bump);
    return `${i ? "L" : "M"}${i * 3} ${(baseline - height).toFixed(2)}`;
  }).join("");
}

export function Wavetable({ directories }: { directories: readonly DirectoryActivity[] | null }) {
  const [shown, setShown] = useState(directories);
  const host = useRef<HTMLDivElement>(null), latest = useRef(directories);
  latest.current = directories;
  const visible = useVisible(host), reduced = useReducedMotion();
  useReadout(host, () => setShown(latest.current), reduced && shown !== directories);
  useEffect(() => {
    // A missing source is not stale activity: clear it immediately.
    if (directories === null) { setShown(null); return; }
    if (!visible || reduced || shown === directories) return;
    const timer = setTimeout(() => setShown(directories), 500);
    return () => clearTimeout(timer);
  }, [directories, visible, reduced, shown]);
  const current = directories === null ? null : shown;
  const ridges = directoryRidges(current ?? []);
  const hits = ridges.reduce((sum, d) => sum + d.hits.reduce((n, h) => n + (usable(h) ? h.hits : 0), 0), 0);
  const omitted = ridges.reduce((sum, d) => sum + d.hits.filter(h => !usable(h)).length, 0);
  return <Lcd ref={host} className="synth-graph synth-wavetable" style={signalStyle("read")}>
    <div className="synth-graph__heading"><span className="synth-silk">Directory activity</span><span>{current === null ? "Unavailable" : `${hits} hits`}</span></div>
    <svg viewBox="0 0 300 150" preserveAspectRatio="none" aria-hidden="true">
      {ridges.map((ridge, index) => {
        const y = 30 + index * (110 / Math.max(1, ridges.length - 1));
        const d = ridgePath(ridge.hits, y);
        return <g key={JSON.stringify(ridge.names)}><path className="synth-wavetable__cover" d={`${d} L300 150L0 150Z`} />
          <path className="synth-wavetable__ridge" d={d} vectorEffect="non-scaling-stroke" /></g>;
      })}
    </svg>
    {current?.length === 0 && <span className="synth-muted">No directories</span>}
    {omitted > 0 && <p className="synth-muted">{omitted} samples omitted: position or hit count unavailable</p>}
    {ridges.length > 0 && <details><summary>{ridges.flatMap(d => d.names).length} directories · {ridges.length} ridges</summary><ul>
      {ridges.map(d => <li key={JSON.stringify(d.names)}>{d.names.length > 1 ? `Other: ${d.names.join(", ")}` : d.names[0]} · {d.hits.reduce((n, h) => n + (usable(h) ? h.hits : 0), 0)} hits</li>)}
    </ul></details>}
  </Lcd>;
}
