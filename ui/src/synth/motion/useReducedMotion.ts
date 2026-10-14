import { useSyncExternalStore } from "react";

const QUERY = "(prefers-reduced-motion: reduce)";
function reduced() { return window.matchMedia(QUERY).matches; }
function subscribe(changed: () => void) {
  const media = window.matchMedia(QUERY);
  media.addEventListener("change", changed);
  return () => media.removeEventListener("change", changed);
}
export function useReducedMotion(): boolean {
  return useSyncExternalStore(subscribe, reduced, () => true);
}
