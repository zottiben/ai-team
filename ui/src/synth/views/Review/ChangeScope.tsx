import { useMemo } from "react";
import type { FileDiff } from "../../../api";
import { Lcd } from "../../primitives/Lcd";
import { Plate } from "../../primitives/Plate";
import { barHeight, hunkBars, type HunkBars } from "./data";

function Bars({ hunks, width, height }: { hunks: readonly HunkBars[]; width: number; height: number }) {
  const step = width / Math.max(1, hunks.length * 4), mid = height / 2;
  return <>{hunks.map((hunk, index) => <g key={index}><title>{hunk.header} · {hunk.added} additions, {hunk.removed} deletions</title>
    {hunk.bars.map((bar, slot) => {
      const x = (index * 4 + slot) * step, up = barHeight(bar.added, Math.min(24, mid - 2)), down = barHeight(bar.removed, Math.min(24, mid - 2));
      return <g key={slot}>{up > 0 && <rect className="wave-added" x={x} y={mid - up} width={step * .7} height={up}/>}
        {down > 0 && <rect className="wave-removed" x={x} y={mid} width={step * .7} height={down}/>}</g>;
    })}</g>)}</>;
}
export function FileWave({ file }: { file: FileDiff }) {
  const hunks = useMemo(() => hunkBars(file).slice(0, 3), [file]);
  return <svg className="synth-review-wave synth-review-mini" width={56} height={20} viewBox="0 0 56 20" role="img" aria-label={file.binary ? "Binary diff: no text waveform" : `${file.path}: first ${hunks.length} displayed hunks`}>
    {file.binary || !hunks.length ? <text x={24} y={13}>—</text> : <Bars hunks={hunks} width={56} height={20}/>}</svg>;
}
export function ChangeScope({ files, viewed, onSelect }: {
  files: readonly FileDiff[]; viewed: ReadonlySet<string>; onSelect: (path: string) => void;
}) {
  const columns = useMemo(() => files.map(file => ({ file, hunks: hunkBars(file) })), [files]);
  const hunks = columns.reduce((total, column) => total + column.hunks.length, 0);
  const read = files.filter(file => viewed.has(file.path)).length;
  return <Plate tone="midnight" className="synth-review-scope" heading="Change scope · unstaged">
    {!files.length ? <p className="synth-review-empty">No unstaged files in this snapshot.</p> : <Lcd>
      <svg className="synth-review-wave" width={files.length * 60} height={62} viewBox={`0 0 ${files.length * 60} 62`} role="img" aria-label="Displayed unstaged hunk additions above the centre; deletions below">
        {columns.map(({ file, hunks }, index) => <g key={file.path} transform={`translate(${index * 60 + 4}, 4)`}>
          <title>{file.path} · {hunks.length} displayed hunks{file.binary ? " · binary" : ""}</title>
          <path className="wave-divider" d="M-4 0 V54 M0 27 H52"/>
          {file.binary || !hunks.length ? <text x={22} y={30}>—</text> : <Bars hunks={hunks} width={52} height={54}/>}
        </g>)}
      </svg>
      <div className="synth-review-pads" aria-label="Viewed unstaged files">{files.map((file, index) => <button key={file.path} type="button"
        data-viewed={viewed.has(file.path)} title={file.path} aria-label={`Show ${file.path}${viewed.has(file.path) ? " · viewed" : " · not viewed"}`}
        onClick={() => onSelect(file.path)}>{index + 1}</button>)}</div>
    </Lcd>}
    <footer>{files.length} files · {hunks} displayed hunks · {read}/{files.length} viewed</footer>
    <p className="faint synth-review-explainer">Four line-count bins per displayed hunk; heights cap at 24px. No bars means no displayed additions or deletions—not an unchanged file. Viewed is personal review progress, never approval.</p>
  </Plate>;
}
