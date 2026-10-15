import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { DiffLine, FileDiff } from "../../../api";
import { useMeasuredRows } from "../../virtual/useMeasuredRows";
import { useDiffSyntax } from "./syntax/useDiffSyntax";
import { CHUNK_ROWS, type Span } from "./syntax/types";

export type DiffRow = { key: string; kind: "header"; header: string }
  | { key: string; kind: "line"; line: DiffLine };
export function diffRows(file: FileDiff): DiffRow[] {
  return file.hunks.flatMap((hunk, h): DiffRow[] => [{ key: `h${h}`, kind: "header", header: hunk.header },
    ...hunk.lines.map((line, n): DiffRow => ({ key: `h${h}l${n}`, kind: "line", line }))]);
}
const keyOf = (row: DiffRow) => row.key;

/** The caller owns the comment draft. Measurement owns only placement, never an anchor. */
export function DiffView({ rows, path, source, synth, pinned, renderLine }: {
  rows: readonly DiffRow[]; path: string; source: string; synth: boolean; pinned: string | null;
  renderLine: (row: Extract<DiffRow, { kind: "line" }>, syntax?: Span[]) => ReactNode;
}) {
  const scroll = useRef<HTMLDivElement>(null), elements = useRef(new Map<string, HTMLDivElement>());
  const [focused, setFocused] = useState<string | null>(null), [pending, setPending] = useState<string | null>(null);
  const [side, setSide] = useState<"old" | "new">("new"), [line, setLine] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const count = useMemo(() => rows.filter(row => row.kind === "line").length, [rows]);
  const windowed = synth && count > 400;
  const keys = [pinned, focused, pending].filter((key): key is string => key !== null);
  const { virtualizer, entries } = useMeasuredRows(rows, keyOf, scroll, windowed, keys, 24);
  const syntax = useDiffSyntax(rows, path, source, entries, synth, scroll);
  useLayoutEffect(() => {
    if (!pending) return;
    const index = rows.findIndex(row => row.key === pending), element = elements.current.get(pending);
    if (index < 0) { setPending(null); return; }
    if (windowed) virtualizer.scrollToIndex(index, { align: "center" });
    if (!element) return;
    const target = element.querySelector<HTMLTextAreaElement>("textarea") ?? element.querySelector<HTMLButtonElement>(".diff__add") ?? element;
    target.scrollIntoView({ block: "nearest" });
    target.focus({ preventScroll: true });
    setPending(null);
  }, [pending, rows, virtualizer, windowed, entries]);
  return <>
    {synth && <form className="synth-diff-navigation" onSubmit={event => {
      event.preventDefault();
      const number = Number(line), row = rows.find(row => row.kind === "line" && row.line[side] === number);
      if (!Number.isSafeInteger(number) || number < 1 || !row) { setProblem("That line is not in the displayed diff hunks."); return; }
      setProblem(null); setPending(row.key);
    }}>
      <span>{count.toLocaleString()} displayed lines{windowed ? " · windowed" : ""}</span>
      <span className="synth-diff-syntax-note" title="Syntax hints use visible 64-row chunks with up to 64 preceding displayed rows. Hunk gaps reset context. Missing file context can change interpretation; this is not validation.">
        {syntax.state === "unsupported" ? "Plain text · no grammar" : syntax.state === "unavailable" ? "Plain text · syntax unavailable" : syntax.state === "limited" ? "Some syntax hints omitted by limits" : `Syntax hints · ${syntax.state === "loading" ? "loading" : "bounded context"}`}
      </span>
      {syntax.state === "unavailable" && <button type="button" className="button" onClick={syntax.retry}>Retry syntax hints</button>}
      <label>Side<select aria-label={`Line side for ${path}`} value={side} onChange={event => setSide(event.target.value === "old" ? "old" : "new")}><option value="new">New</option><option value="old">Old</option></select></label>
      <label>Line<input aria-label={`Go to line in ${path}`} type="number" min={1} step={1} value={line} onChange={event => setLine(event.target.value)}/></label>
      <button type="submit" className="button">Go</button>
      {pinned && <button type="button" className="button" onClick={() => setPending(pinned)}>Show comment</button>}
      {problem && <p role="status">{problem}</p>}
    </form>}
    <div ref={scroll} className="synth-diff-scroll" data-windowed={windowed} role="region" aria-label={`Diff for ${path}`} tabIndex={windowed ? 0 : undefined}
      onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null); }}>
      <div className="synth-diff-rows" style={windowed ? { height: virtualizer.getTotalSize(), position: "relative" } : undefined}>
        {entries.map(entry => {
          const row = rows[entry.index];
          if (!row) return null;
          return <div key={entry.key} data-index={entry.index} data-diff-row={row.key} className="synth-diff-row" tabIndex={-1}
            ref={element => {
              if (element) elements.current.set(row.key, element); else elements.current.delete(row.key);
              if (windowed) virtualizer.measureElement(element);
            }}
            style={windowed ? { position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${entry.start ?? 0}px)` } : undefined}
            onFocusCapture={() => setFocused(row.key)}>
            {row.kind === "header" ? <div className="diff__hunk-head mono">{row.header}</div> : renderLine(row, synth ? syntax.chunks?.get(Math.floor(entry.index / CHUNK_ROWS))?.lines[row.key] : undefined)}
          </div>;
        })}
      </div>
    </div>
  </>;
}
