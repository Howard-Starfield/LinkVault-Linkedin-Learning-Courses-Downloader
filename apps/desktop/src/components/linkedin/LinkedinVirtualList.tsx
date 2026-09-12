import { useVirtualizer } from "@tanstack/react-virtual";
import type { ReactNode, RefObject } from "react";
import { LINKEDIN_HISTORY_OVERSCAN, LINKEDIN_HISTORY_ROW_PX } from "../../lib/linkedin/browse-window";

type LinkedinVirtualListProps<T> = {
  items: readonly T[];
  scrollRef: RefObject<HTMLDivElement | null>;
  ariaLabel: string;
  getKey: (item: T, index: number) => string;
  estimateSize?: (index: number) => number;
  renderItem: (item: T, index: number) => ReactNode;
};

export function LinkedinVirtualList<T>({
  items,
  scrollRef,
  ariaLabel,
  getKey,
  estimateSize,
  renderItem
}: LinkedinVirtualListProps<T>) {
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: estimateSize ?? (() => LINKEDIN_HISTORY_ROW_PX),
    overscan: LINKEDIN_HISTORY_OVERSCAN,
    getItemKey: (index) => getKey(items[index], index)
  });

  return (
    <ol
      className="linkedin-library-list linkedin-library-virtual"
      aria-label={ariaLabel}
      style={{ height: `${virtualizer.getTotalSize()}px` }}
    >
      {virtualizer.getVirtualItems().map((row) => (
        <li
          key={row.key}
          data-index={row.index}
          style={{
            height: `${row.size}px`,
            transform: `translateY(${row.start}px)`
          }}
        >
          {renderItem(items[row.index], row.index)}
        </li>
      ))}
    </ol>
  );
}
