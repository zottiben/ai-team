import { useLayoutEffect, useState, type RefObject } from "react";

/** Offscreen is still owned. A hidden document/pane is not. */
export function displayed(node: HTMLElement | null) {
  if (document.hidden || !node?.isConnected || !node.getClientRects().length) return false;
  for (let parent: HTMLElement | null = node; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    if (parent.hidden || style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse") return false;
  }
  return true;
}
export function useDisplayed(ref: RefObject<HTMLElement | null>) {
  const [value, setValue] = useState(false);
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const update = () => setValue(displayed(node)), observer = new MutationObserver(update);
    // Read after actual owner visibility changes, not root theme/data flags:
    // measuring the intermediate skin before React hides it can clamp scroll.
    for (let parent: HTMLElement | null = node; parent; parent = parent.parentElement) observer.observe(parent, { attributes: true, attributeFilter: ["hidden", "style", "class", "open"] });
    document.addEventListener("visibilitychange", update); window.addEventListener("resize", update); update();
    return () => { observer.disconnect(); document.removeEventListener("visibilitychange", update); window.removeEventListener("resize", update); };
  }, [ref]);
  return value;
}
