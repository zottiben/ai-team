import { useCallback, useMemo, type RefObject } from "react";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/react-virtual";

/** Real per-element measurements, stable keys and a pinned focused row. No type-based
 * height cache: Markdown, output and attachments can all change an individual row. */
export function useMeasuredRows<T>(items: readonly T[], keyOf: (item: T) => string | number,
  scroll: RefObject<HTMLDivElement | null>, enabled: boolean, focused: string | number | null = null) {
  const indices = useMemo(() => new Map(items.map((item, index) => [keyOf(item), index])), [items, keyOf]);
  const pinned = focused === null ? undefined : indices.get(focused);
  const rangeExtractor = useCallback((range: Parameters<typeof defaultRangeExtractor>[0]) => {
    const normal = defaultRangeExtractor(range);
    return pinned === undefined || normal.includes(pinned) ? normal : [...normal, pinned].sort((a, b) => a - b);
  }, [pinned]);
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: items.length, getScrollElement: () => scroll.current, enabled,
    estimateSize: () => 96, overscan: 5, rangeExtractor,
    getItemKey: index => { const item = items[index]; return item === undefined ? index : keyOf(item); },
  });
  return { virtualizer, entries: enabled ? virtualizer.getVirtualItems().map(item => ({ index: item.index, key: item.key, start: item.start }))
    : items.map((item, index) => ({ index, key: keyOf(item), start: undefined })) };
}
