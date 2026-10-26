export type FiniteTarget = { element: HTMLElement; frames: Keyframe[]; duration: number; kind: "receipt" | "pattern" };

/** Includes clipped scroll ancestors, not just presence in the document. */
export function inViewport(element: HTMLElement, owner: HTMLElement): boolean {
  if (!owner.isConnected || !owner.contains(element) || !element.getClientRects().length || document.hidden) return false;
  const box = element.getBoundingClientRect();
  let left = 0, top = 0, right = window.innerWidth, bottom = window.innerHeight;
  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (node.hidden || style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse") return false;
    if (node !== element && (node === owner || /auto|scroll|hidden|clip/.test(style.overflow + style.overflowX + style.overflowY))) {
      const clip = node.getBoundingClientRect();
      left = Math.max(left, clip.left); right = Math.min(right, clip.right); top = Math.max(top, clip.top); bottom = Math.min(bottom, clip.bottom);
    }
  }
  return box.width > 0 && box.height > 0 && right > left && bottom > top && box.right > left && box.left < right && box.bottom > top && box.top < bottom;
}

/** Bounded, one-shot native effects: no ticker, per-frame React state, focus,
 * scrolling, clone or durable marker. New input/hidden ownership cancels them. */
export function finiteMotion(owner: HTMLElement, targets: readonly FiniteTarget[]): () => void {
  const preference = globalThis.matchMedia?.("(prefers-reduced-motion: reduce)");
  if (preference?.matches || document.hidden) return () => {};
  const active = new Map<Animation, HTMLElement>();
  let disposed = false;
  const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => {
    for (const entry of entries) if (!entry.isIntersecting) for (const [animation, element] of active) if (element === entry.target) retire(animation);
  });
  const clear = () => {
    if (disposed) return; disposed = true;
    observer?.disconnect(); document.removeEventListener("visibilitychange", visibility);
    for (const name of ["pointerdown", "keydown", "wheel", "touchstart", "focusin"]) document.removeEventListener(name, cancel, true);
    document.removeEventListener("scroll", scroll, true); window.removeEventListener("resize", cancel); preference?.removeEventListener?.("change", reduce);
  };
  const retire = (animation: Animation) => {
    const element = active.get(animation); if (!element) return;
    active.delete(animation); observer?.unobserve(element); animation.onfinish = null; animation.oncancel = null; animation.cancel();
    if (!active.size) clear();
  };
  const cancel = () => { for (const animation of active.keys()) retire(animation); clear(); };
  const visibility = () => { if (document.hidden) cancel(); };
  const reduce = () => { if (preference?.matches) cancel(); };
  const scroll = () => { for (const [animation, element] of active) if (!inViewport(element, owner)) retire(animation); };
  const easing = getComputedStyle(owner).getPropertyValue("--ease-standard").trim() || "ease-out";
  for (const { element, frames, duration, kind } of targets.slice(0, 256)) {
    if (active.size >= 32) break;
    if ([...active.values()].includes(element)) continue;
    if (typeof element.animate !== "function" || !Number.isFinite(duration) || duration <= 0 || duration > 600 || !inViewport(element, owner)) continue;
    try {
      const animation = element.animate(frames, { duration, easing, iterations: 1, fill: "none" });
      animation.id = `synth-${kind}-arrival`; active.set(animation, element);
      animation.onfinish = () => retire(animation); animation.oncancel = () => retire(animation); observer?.observe(element);
    } catch (error) { console.warn("Receipt motion unavailable; recorded content remains visible", error); }
  }
  if (!active.size) { clear(); return cancel; }
  document.addEventListener("visibilitychange", visibility);
  for (const name of ["pointerdown", "keydown", "wheel", "touchstart", "focusin"]) document.addEventListener(name, cancel, { capture: true, passive: true });
  document.addEventListener("scroll", scroll, { capture: true, passive: true }); window.addEventListener("resize", cancel); preference?.addEventListener?.("change", reduce);
  return cancel;
}
