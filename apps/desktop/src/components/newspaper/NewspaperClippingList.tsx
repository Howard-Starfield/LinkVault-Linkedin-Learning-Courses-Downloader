import { listen } from "@tauri-apps/api/event";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { preserveStableClippingThumbnail } from "./clipping-thumbnail-state";
import {
  ensureNewspaperClippingThumbnail,
  getNewspaperClippingsPage,
  isTauriRuntime,
  type NewspaperClippingSummary
} from "./newspaper-api";

const PAGE_SIZE = 50;
const GRID_GAP = 16;

function columnCountForWidth(width: number) {
  if (width < 560) return 1;
  if (width < 820) return 2;
  if (width < 1080) return 3;
  if (width < 1480) return 4;
  if (width < 1780) return 5;
  return 6;
}

function clippingAspectRatio(item: NewspaperClippingSummary | undefined) {
  if (!item || item.assetWidth <= 0 || item.assetHeight <= 0) return 1.45;
  return Math.min(2.4, Math.max(0.72, item.assetWidth / item.assetHeight));
}

export function NewspaperClippingList({ onSelect }: { onSelect: (id: string) => void }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const generationRef = useRef(0);
  const loadingRef = useRef(new Set<string>());
  const thumbnailRef = useRef(new Set<string>());
  const thumbnailErrorRef = useRef(new Set<string>());
  const mountedRef = useRef(true);
  const [items, setItems] = useState<Array<NewspaperClippingSummary | undefined>>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");
  const [initialLoading, setInitialLoading] = useState(true);
  const [viewportWidth, setViewportWidth] = useState(1200);
  const columnCount = columnCountForWidth(viewportWidth);
  const rowCount = Math.ceil(total / columnCount);

  const loadPage = useCallback(async (offset: number, generation: number, reset = false) => {
    const owner = `${generation}:${offset}`;
    if (loadingRef.current.has(owner)) return;
    loadingRef.current.add(owner);
    try {
      const page = await getNewspaperClippingsPage({ query: "", sort: "updated_desc", offset, limit: PAGE_SIZE });
      if (generation !== generationRef.current) return;
      setError("");
      setTotal(page.total);
      setItems((current) => {
        const next = reset ? new Array<NewspaperClippingSummary | undefined>(page.total) : [...current];
        const previousById = new Map(current.flatMap((item) => item ? [[item.id, item] as const] : []));
        next.length = page.total;
        page.items.forEach((item, index) => {
          next[page.offset + index] = preserveStableClippingThumbnail(item, previousById.get(item.id));
        });
        return next;
      });
    } catch (cause) {
      if (generation === generationRef.current) setError(String(cause));
    } finally {
      loadingRef.current.delete(owner);
      if (offset === 0 && generation === generationRef.current) setInitialLoading(false);
    }
  }, []);

  const refresh = useCallback(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    setItems([]);
    setTotal(0);
    setInitialLoading(true);
    void loadPage(0, generation, true);
  }, [loadPage]);

  const refreshLoadedPages = useCallback(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    const offsets = new Set<number>([0]);
    items.forEach((item, index) => {
      if (item) offsets.add(Math.floor(index / PAGE_SIZE) * PAGE_SIZE);
    });
    offsets.forEach((offset) => void loadPage(offset, generation, false));
  }, [items, loadPage]);

  useEffect(refresh, [refresh]);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const updateWidth = () => setViewportWidth(element.clientWidth);
    updateWidth();
    const observer = new ResizeObserver(updateWidth);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("newspaper://clipping-invalidated", () => {
      if (!disposed) refreshLoadedPages();
    }).then((cleanup) => { unlisten = cleanup; });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshLoadedPages]);

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => Math.max(180, (viewportWidth - (columnCount + 1) * GRID_GAP) / columnCount / 1.35 + GRID_GAP),
    getItemKey: (rowIndex) => `clipping-grid-row-${rowIndex}-${columnCount}`,
    overscan: 2
  });
  const virtualRows = virtualizer.getVirtualItems();
  const visibleItemIndexes = useMemo(() => virtualRows.flatMap((row) => {
    const start = row.index * columnCount;
    return Array.from({ length: columnCount }, (_, column) => start + column).filter((index) => index < total);
  }), [columnCount, total, virtualRows]);

  useEffect(() => {
    if (!visibleItemIndexes.length) return;
    const generation = generationRef.current;
    const offsets = new Set<number>();
    for (const index of visibleItemIndexes) {
      if (!items[index]) offsets.add(Math.floor(index / PAGE_SIZE) * PAGE_SIZE);
    }
    const last = visibleItemIndexes[visibleItemIndexes.length - 1] ?? 0;
    const nextOffset = Math.floor((last + columnCount * 2) / PAGE_SIZE) * PAGE_SIZE;
    if (nextOffset < total && !items[nextOffset]) offsets.add(nextOffset);
    offsets.forEach((offset) => void loadPage(offset, generation));
  }, [columnCount, items, loadPage, total, visibleItemIndexes]);

  useEffect(() => {
    for (const index of visibleItemIndexes) {
      const item = items[index];
      if (!item || item.thumbnailReady || thumbnailRef.current.has(item.id) || thumbnailErrorRef.current.has(item.id)) continue;
      thumbnailRef.current.add(item.id);
      void ensureNewspaperClippingThumbnail(item.id)
        .then((thumbnail) => {
          if (!mountedRef.current) return;
          setItems((current) => current.map((candidate) => candidate?.id === item.id
            ? { ...candidate, thumbnailReady: true, thumbnailUrl: thumbnail.thumbnailUrl, thumbnailVersion: thumbnail.thumbnailVersion }
            : candidate));
        })
        .catch(() => {
          thumbnailErrorRef.current.add(item.id);
          if (!mountedRef.current) return;
          setItems((current) => current.map((candidate) => candidate?.id === item.id
            ? { ...candidate, thumbnailReady: false, thumbnailUrl: null, thumbnailVersion: null }
            : candidate));
        })
        .finally(() => thumbnailRef.current.delete(item.id));
    }
  }, [items, visibleItemIndexes]);

  return (
    <section className="clipping-gallery" aria-label="Saved clippings">
      <header className="clipping-gallery__header">
        <div>
          <span>Saved evidence</span>
          <h2>Clippings</h2>
          <p>Open a clipping to view its source and write a note.</p>
        </div>
        <strong>{total} clipping{total === 1 ? "" : "s"}</strong>
      </header>
      <div className="clipping-gallery__scroll" ref={scrollRef} data-testid="newspaper-clipping-list-scroll">
        {error ? <div className="clipping-gallery__message" role="alert">Could not load clippings. {error}</div> : null}
        {!error && initialLoading ? <div className="clipping-gallery__message">Loading clippings…</div> : null}
        {!error && !initialLoading && total === 0 ? <div className="clipping-gallery__message">Crop an area in Newspaper library to create your first note.</div> : null}
        <div className="clipping-gallery__virtual" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualRows.map((row) => {
            const start = row.index * columnCount;
            return (
              <div
                className="clipping-gallery__row"
                data-index={row.index}
                key={row.key}
                ref={virtualizer.measureElement}
                style={{ gridTemplateColumns: `repeat(${columnCount}, minmax(0, 1fr))`, transform: `translateY(${row.start}px)` }}
              >
                {Array.from({ length: columnCount }, (_, column) => {
                  const item = items[start + column];
                  if (start + column >= total) return null;
                  return (
                    <button
                      aria-label={item ? `Open ${item.title}` : "Loading clipping"}
                      className="clipping-gallery__card"
                      disabled={!item}
                      key={item?.id ?? `clipping-placeholder-${start + column}`}
                      onClick={() => item && onSelect(item.id)}
                      style={{ "--clipping-aspect": clippingAspectRatio(item) } as CSSProperties}
                      type="button"
                    >
                      <span className="clipping-gallery__thumb">
                        {item?.thumbnailReady && item.thumbnailUrl ? (
                          <img
                            alt=""
                            decoding="async"
                            onError={() => {
                              thumbnailErrorRef.current.add(item.id);
                              setItems((current) => current.map((candidate) => candidate?.id === item.id
                                ? { ...candidate, thumbnailReady: false, thumbnailUrl: null, thumbnailVersion: null }
                                : candidate));
                            }}
                            onLoad={(event) => { event.currentTarget.dataset.loaded = "true"; }}
                            src={item.thumbnailUrl}
                          />
                        ) : (
                          <span
                            aria-label={item?.assetState === "missing" || (item && thumbnailErrorRef.current.has(item.id)) ? "Clipping preview unavailable" : "Preparing clipping preview"}
                            className="clipping-gallery__placeholder"
                            role="img"
                          />
                        )}
                        {item ? <span className="clipping-gallery__title"><strong>{item.title}</strong></span> : null}
                      </span>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}
