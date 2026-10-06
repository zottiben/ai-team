import { useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";

/** Viewport-relative placement keeps scrollable sidebars from clipping the panel. */
export function popoverPosition(anchor: Pick<DOMRect, "top" | "right">, width: number, height: number, viewportWidth: number, viewportHeight: number) {
  const gap = 8;
  width = Math.min(width, Math.max(0, viewportWidth - gap * 2));
  const beside = anchor.right + gap;
  return {
    width,
    maxHeight: Math.max(0, viewportHeight - gap * 2),
    left: beside + width <= viewportWidth - gap ? beside : Math.max(gap, Math.min(anchor.right - width, viewportWidth - width - gap)),
    top: Math.max(gap, Math.min(anchor.top, viewportHeight - height - gap)),
  };
}

export function Popover({ anchor, label, children, onClose, width = 448, className = "" }: {
  anchor: RefObject<HTMLButtonElement | null>;
  label: string;
  children: ReactNode;
  onClose: () => void;
  width?: number;
  className?: string;
}) {
  const panel = useRef<HTMLElement | null>(null);
  const [position, setPosition] = useState<ReturnType<typeof popoverPosition> | null>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useLayoutEffect(() => {
    const place = () => {
      const rect = anchor.current?.getBoundingClientRect();
      if (!rect) return;
      const next = popoverPosition(rect, width, panel.current?.offsetHeight ?? 0, window.innerWidth, window.innerHeight);
      setPosition(old => old && old.top === next.top && old.left === next.left && old.width === next.width && old.maxHeight === next.maxHeight ? old : next);
    };
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !panel.current?.contains(event.target) && !anchor.current?.contains(event.target)) close.current();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) {
        event.preventDefault();
        close.current();
        anchor.current?.focus();
      }
    };
    place();
    panel.current?.focus({ preventScroll: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    if (panel.current) observer?.observe(panel.current);
    if (anchor.current) observer?.observe(anchor.current);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", key);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", key);
    };
  }, [anchor, width]);
  return createPortal(<section ref={panel} tabIndex={-1} aria-label={label} className={`anchored-popover ${className}`}
    style={position ?? { visibility: "hidden" }}>
    {children}
  </section>, document.body);
}
