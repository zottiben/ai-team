/** A gesture may hand focus to its reason form or settled card, never a newer input. */
export function ownedFocus(contains: (node: Node) => boolean, inForm: (node: Node) => boolean = () => false, initiator: Event | null = null) {
  let last = document.activeElement, closed = false, frame: number | null = null;
  const cleanup = () => {
    document.removeEventListener("focusin", focus); document.removeEventListener("focusout", released); document.removeEventListener("pointerdown", pointer); document.removeEventListener("keydown", key); document.removeEventListener("visibilitychange", visibility); document.removeEventListener("wheel", cancel); document.removeEventListener("touchmove", cancel); window.removeEventListener("blur", cancel); window.removeEventListener("resize", cancel);
  };
  const cancel = () => { closed = true; if (frame !== null) cancelAnimationFrame(frame); frame = null; cleanup(); };
  const focus = (event: FocusEvent) => {
    if (event.target instanceof Node && contains(event.target)) last = event.target instanceof Element ? event.target : last;
    else if (event.target !== document.body) cancel();
  };
  const released = (event: FocusEvent) => {
    if (event.target === last && last?.matches(":disabled") && event.relatedTarget === null) last = document.body;
  };
  const pointer = (event: PointerEvent) => { if (!(event.target instanceof Node) || !inForm(event.target)) cancel(); };
  const key = (event: KeyboardEvent) => { if (event !== initiator && (!(event.target instanceof Node) || !inForm(event.target))) cancel(); };
  const visibility = () => { if (document.hidden) cancel(); };
  document.addEventListener("focusin", focus); document.addEventListener("focusout", released); document.addEventListener("pointerdown", pointer); document.addEventListener("keydown", key); document.addEventListener("visibilitychange", visibility); document.addEventListener("wheel", cancel, { passive: true }); document.addEventListener("touchmove", cancel, { passive: true }); window.addEventListener("blur", cancel); window.addEventListener("resize", cancel);
  const owned = () => {
    const current = document.activeElement;
    const released = current === document.body && (last === document.body || !last?.isConnected || last.matches(":disabled") || !last.getClientRects().length);
    return !closed && !document.hidden && (current === last || !!(current && contains(current)) || released);
  };
  const reveal = (target: HTMLElement | null) => {
    if (owned() && target?.getClientRects().length && getComputedStyle(target).visibility === "visible") target.scrollIntoView({ block: "nearest", inline: "nearest" });
  };
  const take = (target: HTMLElement | null) => {
    if (!owned() || !target?.getClientRects().length || target.matches(":disabled") || getComputedStyle(target).visibility !== "visible") return false;
    reveal(target); target.focus({ preventScroll: true }); last = target;
    return document.activeElement === target;
  };
  return {
    cancel, take, reveal,
    restore(find: () => HTMLElement | null, restored?: () => void) {
      if (closed) return;
      frame = requestAnimationFrame(() => { frame = null; try { if (take(find())) restored?.(); } finally { cancel(); } });
    },
  };
}
