export function threePageRange(activeIndex: number, count: number) {
  const result: number[] = [];
  for (const index of [activeIndex - 1, activeIndex, activeIndex + 1]) {
    if (index >= 0 && index < count) result.push(index);
  }
  return result;
}

export function boundedLibraryRange(
  firstVisibleIndex: number,
  visibleRowCount: number,
  itemCount: number,
  overscan: number
) {
  if (itemCount <= 0) return [];
  const start = Math.max(0, firstVisibleIndex - overscan);
  const end = Math.min(
    itemCount,
    firstVisibleIndex + Math.max(0, visibleRowCount) + overscan
  );
  return Array.from({ length: Math.max(0, end - start) }, (_, index) => start + index);
}

export function visibleVirtualIndexes(
  items: Array<{ index: number; start: number; end: number }>,
  scrollOffset: number,
  viewportSize: number
) {
  const viewportEnd = scrollOffset + Math.max(0, viewportSize);
  return new Set(
    items
      .filter((item) => item.end > scrollOffset && item.start < viewportEnd)
      .map((item) => item.index)
  );
}

export function pageOffsetsForRange(indices: number[], pageSize: number) {
  if (pageSize <= 0) throw new Error("pageSize must be positive");
  return [...new Set(indices.map((index) => Math.floor(index / pageSize) * pageSize))];
}
