import { useLayoutEffect, useState, type RefObject } from "react";
import { useVisible } from "../../motion/useVisible";

/** Follow the tail until the operator scrolls away or owns keyboard focus. */
export function usePatternViewport(scroll: RefObject<HTMLDivElement | null>, count: number, latest: number | undefined, focused: boolean) {
  const [width, setWidth] = useState(0), [following, setFollowing] = useState(true);
  const visible = useVisible(scroll);
  useLayoutEffect(() => {
    const element = scroll.current;
    if (!element) return;
    const measure = () => { if (element.clientWidth) setWidth(element.clientWidth); };
    measure();
    const observer = new ResizeObserver(measure); observer.observe(element);
    return () => observer.disconnect();
  }, [scroll]);
  const columnWidth = Math.max(44, width / Math.max(1, count));
  useLayoutEffect(() => {
    const element = scroll.current;
    if (element && following && !focused && visible) element.scrollLeft = Math.max(0, count * columnWidth - element.clientWidth);
  }, [scroll, count, latest, width, columnWidth, following, focused, visible]);
  return { columnWidth, following, follow: () => setFollowing(true), onScroll: () => {
    const element = scroll.current;
    if (!element || focused || !visible || !element.clientWidth) return;
    const next = element.scrollWidth - element.clientWidth - element.scrollLeft < 8;
    setFollowing(next); return next;
  } };
}
