import { useEffect, useState, type RefObject } from "react";

/** Visibility is separate from motion preference: discrete readouts still update. */
export function useVisible(element: RefObject<Element | null>): boolean {
  const [visible, setVisible] = useState(typeof IntersectionObserver === "undefined");
  const [hidden, setHidden] = useState(document.hidden);
  useEffect(() => {
    const visibility = () => setHidden(document.hidden);
    visibility();
    document.addEventListener("visibilitychange", visibility);
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => {
      // A quick hide/reveal can queue both observations before delivery. The first
      // one would leave a visible retained pane disabled until another intersection.
      const entry = entries.filter(e => e.target === element.current).at(-1);
      if (entry) setVisible(entry.isIntersecting);
    });
    if (element.current) observer?.observe(element.current);
    return () => { observer?.disconnect(); document.removeEventListener("visibilitychange", visibility); };
  }, [element]);
  return visible && !hidden;
}
