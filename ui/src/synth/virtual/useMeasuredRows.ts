import { useCallback, useMemo, useRef, type RefObject } from "react";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/react-virtual";

/** Real per-element measurements, stable keys and a pinned focused row. No type-based
 * height cache: Markdown, output and attachments can all change an individual row. */
export function useMeasuredRows<T>(items: readonly T[], keyOf: (item: T) => string | number,
  scroll: RefObject<HTMLDivElement | null>, enabled: boolean,
  focused: string | number | readonly (string | number)[] | null = null, estimateSize = 96) {
  const indices = useMemo(() => new Map(items.map((item, index) => [keyOf(item), index])), [items, keyOf]);
  const keys = typeof focused === "string" || typeof focused === "number" ? [focused] : focused ?? [];
  const pinnedKey = JSON.stringify([...new Set(keys.map(key => indices.get(key)).filter(index => index !== undefined))].sort((a, b) => a - b));
  const pinned = useMemo<number[]>(() => JSON.parse(pinnedKey), [pinnedKey]);
  const rangeExtractor = useCallback((range: Parameters<typeof defaultRangeExtractor>[0]) => {
    return [...new Set([...defaultRangeExtractor(range), ...pinned])].sort((a, b) => a - b);
  }, [pinned]);
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: items.length, getScrollElement: () => scroll.current, enabled,
    estimateSize: () => estimateSize, overscan: 5, rangeExtractor,
    getItemKey: index => { const item = items[index]; return item === undefined ? index : keyOf(item); },
  });
  const visible = virtualizer.getVirtualItems().map(item => ({ index: item.index, key: item.key, start: item.start }));
  const previous = useRef(visible);
  if (visible.length) previous.current = visible;
  // A retained hidden panel can have a zero-height viewport. Keep its explicitly
  // pinned editor/comment DOM, not a whole offscreen window, until measurement resumes.
  const retained = visible.length ? visible : pinned.flatMap(index => {
    const item = items[index];
    if (item === undefined) return [];
    const key = keyOf(item), prior = previous.current.find(entry => entry.key === key);
    return [{ index, key, start: prior?.start ?? 0 }];
  });
  return { virtualizer, entries: enabled ? retained
    : items.map((item, index) => ({ index, key: keyOf(item), start: undefined })) };
}
