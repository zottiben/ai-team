import { useEffect, useMemo, useState, type RefObject } from "react";
import { useVisible } from "../../../motion/useVisible";
import type { DiffRow } from "../DiffView";
import { syntaxClient } from "./client";
import { CHUNK_ROWS, CONTEXT_ROWS, MAX_CHARS, MAX_CHUNKS, type SyntaxInput, type SyntaxResult } from "./types";

/** Bound before creating arrays or structured-cloning anything to the worker. */
export function syntaxInput(rows: readonly DiffRow[], path: string, chunk: number): SyntaxInput | null {
  if (path.length > 4096) return null;
  const start = chunk * CHUNK_ROWS, from = Math.max(0, start - CONTEXT_ROWS), end = Math.min(rows.length, start + CHUNK_ROWS);
  let chars = 0;
  for (let i = from; i < end; i++) {
    const row = rows[i]!; chars += (row.kind === "header" ? row.header : row.line.text).length + 1;
    if (chars > MAX_CHARS) return null;
  }
  return { path, lines: rows.slice(from, end).map((row, offset) => ({ key: row.key, kind: row.kind === "header" ? "header" : row.line.kind,
    text: row.kind === "header" ? row.header : row.line.text, display: from + offset >= start })) };
}
export function useDiffSyntax(rows: readonly DiffRow[], path: string, source: string, entries: readonly { index: number }[], enabled: boolean, host: RefObject<HTMLElement | null>) {
  const visible = useVisible(host), active = enabled && visible;
  const wanted = useMemo(() => [...new Set(entries.map(entry => Math.floor(entry.index / CHUNK_ROWS)))].sort((a, b) => a - b).slice(0, MAX_CHUNKS), [entries]);
  const signature = wanted.join(",");
  const [cache, setCache] = useState<{ source: string; chunks: ReadonlyMap<number, SyntaxResult> }>({ source, chunks: new Map() });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => { if (active) return syntaxClient.retain(); }, [active]);
  useEffect(() => {
    if (!active) return;
    let current = true;
    const cancel: (() => void)[] = [];
    for (const chunk of wanted) {
      if (cache.source === source && cache.chunks.has(chunk)) continue;
      const done = (result: SyntaxResult) => {
        if (!current) return;
        setCache(previous => {
          const chunks = new Map(previous.source === source ? previous.chunks : []);
          chunks.delete(chunk); chunks.set(chunk, result);
          for (const key of chunks.keys()) {
            if (chunks.size <= MAX_CHUNKS) break;
            if (!wanted.includes(key)) chunks.delete(key);
          }
          return { source, chunks };
        });
      };
      const input = syntaxInput(rows, path, chunk);
      if (input) cancel.push(syntaxClient.request(input, done)); else done({ state: "limited", lines: {} });
    }
    return () => { current = false; cancel.forEach(stop => stop()); };
    // Cache arrivals paint only; they must not cancel other pending chunks.
  }, [rows, path, source, active, signature, attempt]);
  const chunks = cache.source === source ? cache.chunks : null;
  const states = wanted.map(key => chunks?.get(key)?.state);
  const state = states.includes("unavailable") ? "unavailable" : states.includes("limited") ? "limited" : states.includes("unsupported") ? "unsupported" : states.includes(undefined) ? "loading" : "ready";
  return { chunks, state, retry: () => { setCache({ source, chunks: new Map() }); setAttempt(n => n + 1); } };
}
