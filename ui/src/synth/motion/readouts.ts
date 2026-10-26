import { useEffect, useRef, type RefObject } from "react";
import { useVisible } from "./useVisible";

/** A single discrete 1Hz clock for text/data, including under reduced motion. Not animation. */
export function createReadoutClock() {
  const readers = new Set<() => void>();
  let timer: ReturnType<typeof setTimeout> | null = null;
  const schedule = () => {
    if (!readers.size || timer !== null) return;
    timer = setTimeout(() => {
      timer = null;
      try { for (const read of [...readers]) if (readers.has(read)) read(); }
      finally { schedule(); }
    }, 1000);
  };
  return (read: () => void) => {
    readers.add(read); schedule();
    return () => {
      readers.delete(read);
      if (!readers.size && timer !== null) { clearTimeout(timer); timer = null; }
    };
  };
}
const subscribe = createReadoutClock();
export function useReadout(element: RefObject<Element | null>, read: () => void, active: boolean) {
  const visible = useVisible(element), callback = useRef(read);
  callback.current = read;
  useEffect(() => {
    if (active && visible) return subscribe(() => callback.current());
  }, [active, visible]);
}
