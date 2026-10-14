import { useEffect, useState, type RefObject } from "react";
import { useReducedMotion } from "./useReducedMotion";

/** CSS-only pulses need the same visibility gates as ticker-driven SVGs. */
export function useMotionVisible(element: RefObject<Element | null>): boolean {
  const reduced = useReducedMotion();
  const [visible, setVisible] = useState(typeof IntersectionObserver === "undefined");
  const [hidden, setHidden] = useState(document.hidden);
  useEffect(() => {
    const visibility = () => setHidden(document.hidden);
    visibility();
    document.addEventListener("visibilitychange", visibility);
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => {
      const entry = entries.find(e => e.target === element.current);
      if (entry) setVisible(entry.isIntersecting);
    });
    if (element.current) observer?.observe(element.current);
    return () => { observer?.disconnect(); document.removeEventListener("visibilitychange", visibility); };
  }, [element]);
  return visible && !hidden && !reduced;
}
