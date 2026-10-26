import { useCallback, useMemo, useState } from "react";
import type { RunFilter } from "./runs";

/**
 * What the reader has chosen in the Work panel, owned above the skin.
 *
 * The flag only changes decoration, so the filter and the prompts a person opened must
 * survive it. Held by `ChatContext`, which both skins render, rather than inside the
 * synth view that one of them unmounts.
 */
export interface WorkView {
  filter: RunFilter;
  setFilter: (filter: RunFilter) => void;
  expanded: ReadonlySet<number>;
  toggleExpanded: (runId: number) => void;
}

export function useWorkView(): WorkView {
  const [filter, setFilter] = useState<RunFilter>("all");
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set());
  const toggleExpanded = useCallback((runId: number) => setExpanded(old => {
    const next = new Set(old);
    if (!next.delete(runId)) next.add(runId);
    return next;
  }), []);
  return useMemo(() => ({ filter, setFilter, expanded, toggleExpanded }), [filter, expanded, toggleExpanded]);
}
