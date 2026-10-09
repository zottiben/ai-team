import { useEffect, useState } from "react";
import type { FileDiff } from "./api";

const STORAGE = "ai-team.review-files.v1";
type Saved = { key: string; hash: string; viewed: boolean; collapsed: boolean };
type Progress = { source: string; storageKey: string | null; hash: string | null; viewed: boolean; collapsed: boolean; edited: boolean };
function read(): Saved[] {
  const value: unknown = JSON.parse(localStorage.getItem(STORAGE) ?? "[]");
  if (!Array.isArray(value)) return [];
  return value.slice(-500).filter((v): v is Saved => v !== null && typeof v === "object"
    && typeof v.key === "string" && /^[a-f0-9]{64}$/.test(v.key) && typeof v.hash === "string" && /^[a-f0-9]{64}$/.test(v.hash)
    && typeof v.viewed === "boolean" && typeof v.collapsed === "boolean");
}
async function digest(text: string): Promise<string> {
  const buffer = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(buffer), byte => byte.toString(16).padStart(2, "0")).join("");
}

/** Presentation only. Store digests, never source text, review findings or authority. */
export function useFileProgress(file: FileDiff, reviewKey: string | undefined, initiallyCollapsed: boolean, reason?: string | null, revision?: string) {
  const key = JSON.stringify([reviewKey, file.path]);
  const source = JSON.stringify([key, file, reason, revision]);
  const fresh: Progress = { source, storageKey: null, hash: null, viewed: false, collapsed: initiallyCollapsed, edited: false };
  const [progress, setProgress] = useState<Progress>(fresh);
  const [problem, setProblem] = useState<string | null>(null);
  const current = progress.source === source ? progress : fresh;
  useEffect(() => {
    let active = true;
    setProblem(null);
    if (reviewKey) void (async () => {
      const [storageKey, hash] = await Promise.all([digest(key), digest(source)]);
      const saved = read().find(entry => entry.key === storageKey && entry.hash === hash);
      if (active) setProgress(previous => previous.source === source && previous.edited
        ? { ...previous, storageKey, hash }
        : { source, storageKey, hash, viewed: saved?.viewed ?? false, collapsed: saved?.collapsed ?? initiallyCollapsed, edited: false });
    })().catch(() => { if (active) setProblem("Review progress is kept for this view only; local storage is unavailable."); });
    return () => { active = false; };
  }, [key, source, reviewKey, initiallyCollapsed]);
  useEffect(() => {
    if (!reviewKey || progress.source !== source || !progress.storageKey || !progress.hash || !progress.edited) return;
    try {
      const saved: Saved = { key: progress.storageKey, hash: progress.hash, viewed: progress.viewed, collapsed: progress.collapsed };
      localStorage.setItem(STORAGE, JSON.stringify([...read().filter(entry => entry.key !== progress.storageKey).slice(-499), saved]));
    } catch { setProblem("Review progress is kept for this view only; local storage is unavailable."); }
  }, [key, progress, source, reviewKey]);
  const change = (values: Partial<Pick<Progress, "viewed" | "collapsed">>) => setProgress({ ...current, ...values, edited: true });
  return { ...current, problem, toggle: () => change({ collapsed: !current.collapsed }), mark: (viewed: boolean) => change({ viewed, collapsed: viewed }) };
}
