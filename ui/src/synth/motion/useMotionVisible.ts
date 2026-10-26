import type { RefObject } from "react";
import { useReducedMotion } from "./useReducedMotion";
import { useVisible } from "./useVisible";

/** CSS-only pulses need the same visibility gates as ticker-driven SVGs. */
export function useMotionVisible(element: RefObject<Element | null>): boolean {
  const visible = useVisible(element), reduced = useReducedMotion();
  return visible && !reduced;
}
