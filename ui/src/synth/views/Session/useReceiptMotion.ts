import { useLayoutEffect, useRef, type RefObject } from "react";
import type { RunEvent } from "../../../api";
import { finiteMotion, type FiniteTarget } from "../../motion/finite";
import { useMotionVisible } from "../../motion/useMotionVisible";
import { receiptDelta } from "./activity-motion";
import type { TrackerRow } from "./Tracker/rows";

/** `ready` means the controller's initial history snapshot loaded, not runtime health. */
export function useReceiptMotion(rows: readonly TrackerRow[], list: RefObject<HTMLDivElement | null>, viewport: RefObject<HTMLDivElement | null>, enabled: boolean, eventKey?: (event: RunEvent) => string | number, ready = true): boolean {
  const eligible = useMotionVisible(viewport) && enabled && ready;
  const highwater = useRef<number | null>(null), wasReady = useRef(false);
  useLayoutEffect(() => {
    // Follow-up receipt presentation already owns its human arrival. Its later
    // canonical echo must not make the same request appear to arrive twice.
    const initial = !wasReady.current;
    const next = receiptDelta(rows, highwater.current, event => event.actor === "human" && !!eventKey && eventKey(event) !== event.id);
    highwater.current = next.highwater; wasReady.current = ready;
    let disposed = false, cancel = () => {};
    // After the parent's retained follow/measurement layout, before the next paint.
    queueMicrotask(() => {
      const owner = viewport.current, content = list.current;
      if (disposed || initial || !eligible || !owner || !content) return;
      const targets = next.ids.flatMap(id => {
        const element = content.querySelector<HTMLElement>(`[data-read-id="${id}"], [data-thinking-id="${id}"]`) ?? content.querySelector<HTMLElement>(`[data-row-id="${id}"]`);
        return element ? [{ element, frames: [{ opacity: 0, translate: "0 8px" }, { opacity: 1, translate: "0 0" }], duration: 180, kind: "receipt" as const }] : [];
      });
      cancel = finiteMotion(owner, targets);
    });
    return () => { disposed = true; cancel(); };
  }, [rows, list, viewport, eligible, eventKey, ready]);
  return eligible;
}

export function usePatternMotion(rows: readonly TrackerRow[], viewport: RefObject<HTMLDivElement | null>, offset: number, range: number | "all", ready = true) {
  const eligible = useMotionVisible(viewport) && ready;
  const previous = useRef<{ highwater: number; offset: number; range: number | "all"; ready: boolean } | null>(null);
  useLayoutEffect(() => {
    const before = previous.current, next = receiptDelta(rows, before?.highwater ?? null);
    previous.current = { highwater: next.highwater, offset, range, ready };
    let disposed = false, cancel = () => {};
    queueMicrotask(() => {
      const owner = viewport.current;
      if (disposed || !eligible || !owner || !before?.ready || before.range !== range || !next.ids.length) return;
      const targets: FiniteTarget[] = next.ids.flatMap(id => {
        const element = owner.querySelector<HTMLElement>(`[data-pattern-id="${id}"] > button > span`);
        return element ? [{ element, frames: [{ scale: "1.12" }, { scale: "1" }], duration: 160, kind: "pattern" as const }] : [];
      });
      const shift = offset - before.offset;
      if (shift > 0 && shift <= 4) for (const element of owner.querySelectorAll<HTMLElement>("[data-pattern-id]")) {
        // New columns pop at their recorded position; shifting them too can push
        // them beyond the clip and cancel both effects before the first paint.
        if (Number(element.dataset.patternId) > before.highwater) continue;
        targets.push({ element, frames: [{ translate: `${shift * 44}px 0` }, { translate: "0 0" }], duration: 160, kind: "pattern" });
      }
      cancel = finiteMotion(owner, targets);
    });
    return () => { disposed = true; cancel(); };
  }, [rows, viewport, offset, range, eligible, ready]);
}
