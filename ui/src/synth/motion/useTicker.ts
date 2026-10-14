import { useEffect, useRef, type RefObject } from "react";
import { subscribe, type Tick } from "./ticker";
import { useReducedMotion } from "./useReducedMotion";

export function useTicker(element: RefObject<Element | null>, tick: Tick, fps: number, active = true, restart?: unknown) {
  const callback = useRef(tick);
  callback.current = tick;
  const reduced = useReducedMotion();
  useEffect(() => {
    if (!active || reduced || !element.current) return;
    const observable = typeof IntersectionObserver !== "undefined";
    const sub = subscribe((now, dt) => callback.current(now, dt), fps, !observable);
    const observer = observable ? new IntersectionObserver(entries => {
      const entry = entries.find(e => e.target === element.current);
      if (entry) sub.visible(entry.isIntersecting);
    }) : null;
    observer?.observe(element.current);
    return () => { observer?.disconnect(); sub.unsubscribe(); };
  }, [element, fps, active, reduced, restart]);
}
