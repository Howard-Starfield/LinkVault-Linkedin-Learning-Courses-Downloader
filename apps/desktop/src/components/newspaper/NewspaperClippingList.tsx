import { listen } from "@tauri-apps/api/event";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { Button } from "../primitives";
import { preserveStableClippingThumbnail } from "./clipping-thumbnail-state";
import {
  canListenForClippingInstanceActivation,
  listenForClippingInstanceActivation
} from "./clipping-instance-activation";
import {
  CLIPPING_VIEW_MODE_EVENT,
  readClippingViewMode,
  type ClippingViewMode
} from "./clipping-view-preferences";
import {
  ensureNewspaperClippingThumbnail,
  getNewspaperClippingsPage,
  isTauriRuntime,
  type NewspaperClippingSummary
} from "./newspaper-api";

const PAGE_SIZE = 50;
const GRID_GAP = 16;
const MAX_MOUNTED_CARDS = 48;
const LIST_ROW_HEIGHT = 72;

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
  return item.assetWidth / item.assetHeight;
}

function ClippingSkeletonShelf({ loading = false }: { loading?: boolean }) {
  return (
    <div aria-hidden="true" className={`clipping-gallery__skeletons${loading ? " is-loading" : ""}`}>
      {Array.from({ length: 4 }, (_, index) => <span key={index} />)}
    </div>
  );
}

function ClippingEmptyState({ onOpenLibrary }: { onOpenLibrary: () => void }) {
  return (
    <div className="clipping-gallery__empty">
      <div className="clipping-gallery__empty-card">
        <h2>No clippings yet</h2>
        <p>Save clips from Newspaper library and they will show up here with their notes.</p>
        <Button onClick={onOpenLibrary} size="sm" variant="primary">Open Newspaper library</Button>
      </div>
    </div>
  );
}

function clippingMeta(item: NewspaperClippingSummary) {
  return [item.editionName, item.publicationDate, item.pageNumber ? `p. ${item.pageNumber}` : null]
    .filter(Boolean)
    .join(" · ");
}

export function NewspaperClippingList({
  hidden,
  initialScrollTop,
  onOpenLibrary,
  onOrderedIdsChange,
  onScrollTopChange,
  onSelect,
  onSummaryChange
}: {
  hidden: boolean;
  initialScrollTop: number;
  onOpenLibrary: () => void;
  onOrderedIdsChange: (ids: string[]) => void;
  onScrollTopChange: (scrollTop: number) => void;
  onSelect: (id: string) => void;
  onSummaryChange: (summary: { total: number; loading: boolean } | null) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const generationRef = useRef(0);
  const loadingRef = useRef(new Set<string>());
  const thumbnailRef = useRef(new Set<string>());
  const thumbnailErrorRef = useRef(new Set<string>());
  const mountedRef = useRef(true);
  const scrollRestoredRef = useRef(false);
  const [viewMode, setViewMode] = useState<ClippingViewMode>(() => readClippingViewMode());
  const [items, setItems] = useState<Array<NewspaperClippingSummary | undefined>>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");
  const [initialLoading, setInitialLoading] = useState(true);
  const [viewportWidth, setViewportWidth] = useState(1200);
  const columnCount = columnCountForWidth(viewportWidth);
  const galleryRowCount = Math.ceil(total / columnCount);
  const isGallery = viewMode === "gallery";

  useEffect(() => {
    const handleViewMode = (event: Event) => {
      const detail = (event as CustomEvent<ClippingViewMode>).detail;
      if (detail === "gallery" || detail === "list") setViewMode(detail);
      else setViewMode(readClippingViewMode());
    };
    window.addEventListener(CLIPPING_VIEW_MODE_EVENT, handleViewMode);
    return () => window.removeEventListener(CLIPPING_VIEW_MODE_EVENT, handleViewMode);
  }, []);

  const viewModeReadyRef = useRef(false);
  useEffect(() => {
    if (!viewModeReadyRef.current) {
      viewModeReadyRef.current = true;
      return;
    }
    const element = scrollRef.current;
    if (!element) return;
    element.scrollTop = 0;
    onScrollTopChange(0);
  }, [viewMode, onScrollTopChange]);

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
    onSummaryChange({ total, loading: initialLoading });
  }, [initialLoading, onSummaryChange, total]);

  useEffect(() => {
    onOrderedIdsChange(items.flatMap((item) => item ? [item.id] : []));
  }, [items, onOrderedIdsChange]);

  useEffect(() => () => {
    onOrderedIdsChange([]);
    onSummaryChange(null);
  }, [onOrderedIdsChange, onSummaryChange]);

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
    const element = scrollRef.current;
    if (!element) return;
    const reportScroll = () => onScrollTopChange(element.scrollTop);
    element.addEventListener("scroll", reportScroll, { passive: true });
    return () => {
      reportScroll();
      element.removeEventListener("scroll", reportScroll);
    };
  }, [onScrollTopChange]);

  useEffect(() => {
    if (initialLoading || scrollRestoredRef.current) return;
    scrollRestoredRef.current = true;
    const frame = window.requestAnimationFrame(() => {
      if (scrollRef.current) scrollRef.current.scrollTop = initialScrollTop;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [initialLoading, initialScrollTop]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const refreshFromBackend = () => {
      if (!disposed) refreshLoadedPages();
    };
    const retainCleanup = (registration: Promise<() => void>) => {
      void registration.then((cleanup) => {
        if (disposed) cleanup();
        else unlisteners.push(cleanup);
      });
    };
    retainCleanup(listen("newspaper://clipping-invalidated", refreshFromBackend));
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [refreshLoadedPages]);

  useEffect(() => {
    if (!canListenForClippingInstanceActivation()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenForClippingInstanceActivation(() => {
      if (!disposed) refreshLoadedPages();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshLoadedPages]);

  const galleryVirtualizer = useVirtualizer({
    count: isGallery ? galleryRowCount : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => Math.max(180, (viewportWidth - (columnCount + 1) * GRID_GAP) / columnCount / 1.45 + GRID_GAP),
    getItemKey: (rowIndex) => `clipping-grid-row-${rowIndex}-${columnCount}`,
    overscan: 2
  });
  const listVirtualizer = useVirtualizer({
    count: isGallery ? 0 : total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => LIST_ROW_HEIGHT,
    getItemKey: (index) => items[index]?.id ?? `clipping-list-${index}`,
    overscan: 6
  });

  const galleryVirtualRows = galleryVirtualizer.getVirtualItems();
  const listVirtualRows = listVirtualizer.getVirtualItems();
  const boundedVirtualRows = useMemo(() => {
    const maxRows = Math.max(1, Math.ceil(MAX_MOUNTED_CARDS / columnCount));
    const scrollTop = galleryVirtualizer.scrollOffset ?? scrollRef.current?.scrollTop ?? 0;
    const viewportHeight = scrollRef.current?.clientHeight ?? 960;
    const rowSize = galleryVirtualRows[0]?.size ?? 180;
    const nearViewport = galleryVirtualRows.filter((row) => (
      row.end >= scrollTop - rowSize * 2
      && row.start <= scrollTop + viewportHeight + rowSize * 2
    ));
    return (nearViewport.length ? nearViewport : galleryVirtualRows).slice(0, maxRows);
  }, [columnCount, galleryVirtualRows, galleryVirtualizer.scrollOffset]);

  const visibleItemIndexes = useMemo(() => {
    if (!isGallery) {
      return listVirtualRows.map((row) => row.index).filter((index) => index < total);
    }
    return boundedVirtualRows.flatMap((row) => {
      const start = row.index * columnCount;
      return Array.from({ length: columnCount }, (_, column) => start + column).filter((index) => index < total);
    });
  }, [boundedVirtualRows, columnCount, isGallery, listVirtualRows, total]);

  useEffect(() => {
    if (!visibleItemIndexes.length) return;
    const generation = generationRef.current;
    const offsets = new Set<number>();
    for (const index of visibleItemIndexes) {
      if (!items[index]) offsets.add(Math.floor(index / PAGE_SIZE) * PAGE_SIZE);
    }
    const last = visibleItemIndexes[visibleItemIndexes.length - 1] ?? 0;
    const lookahead = isGallery ? columnCount * 2 : 8;
    const nextOffset = Math.floor((last + lookahead) / PAGE_SIZE) * PAGE_SIZE;
    if (nextOffset < total && !items[nextOffset]) offsets.add(nextOffset);
    offsets.forEach((offset) => void loadPage(offset, generation));
  }, [columnCount, isGallery, items, loadPage, total, visibleItemIndexes]);

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

  const renderThumb = (item: NewspaperClippingSummary | undefined, compact = false) => (
    <span className={compact ? "clipping-list-row__thumb" : "clipping-gallery__thumb"}>
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
          className={compact ? "clipping-list-row__placeholder" : "clipping-gallery__placeholder"}
          role="img"
        />
      )}
    </span>
  );

  return (
    <section
      className={`clipping-gallery${isGallery ? "" : " clipping-gallery--list"}`}
      aria-label="Saved clippings"
      hidden={hidden}
      data-view-mode={viewMode}
    >
      <div className="clipping-gallery__scroll" ref={scrollRef} data-testid="newspaper-clipping-list-scroll">
        {error ? <div className="clipping-gallery__message" role="alert">Could not load clippings. {error}</div> : null}
        {!error && initialLoading ? (
          <div className="clipping-gallery__loading" role="status">
            <span className="sr-only">Loading clippings…</span>
            {isGallery ? <ClippingSkeletonShelf loading /> : (
              <div className="clipping-list clipping-list--loading" aria-hidden="true">
                {Array.from({ length: 4 }, (_, index) => (
                  <div className="clipping-list-row clipping-list-row--skeleton" key={index} />
                ))}
              </div>
            )}
          </div>
        ) : null}
        {!error && !initialLoading && total === 0 ? (
          <ClippingEmptyState onOpenLibrary={onOpenLibrary} />
        ) : null}
        {isGallery ? (
          <div className="clipping-gallery__virtual" style={{ height: `${galleryVirtualizer.getTotalSize()}px` }}>
            {boundedVirtualRows.map((row) => {
              const start = row.index * columnCount;
              return (
                <div
                  className="clipping-gallery__row"
                  data-index={row.index}
                  key={row.key}
                  ref={galleryVirtualizer.measureElement}
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
                        {renderThumb(item)}
                        {item ? <span className="clipping-gallery__title"><strong>{item.title}</strong></span> : null}
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
        ) : (
          <div className="clipping-list" style={{ height: `${listVirtualizer.getTotalSize()}px` }} aria-label="Clipping list">
            {listVirtualRows.map((row) => {
              const item = items[row.index];
              return (
                <button
                  type="button"
                  key={row.key}
                  className={`clipping-list-row${item ? "" : " clipping-list-row--skeleton"}`}
                  disabled={!item}
                  aria-label={item ? `Open ${item.title}` : "Loading clipping"}
                  onClick={() => item && onSelect(item.id)}
                  style={{ height: `${row.size}px`, transform: `translateY(${row.start}px)` }}
                >
                  {item ? (
                    <>
                      {renderThumb(item, true)}
                      <span className="clipping-list-row__copy">
                        <strong>{item.title}</strong>
                        <span>{clippingMeta(item)}</span>
                      </span>
                    </>
                  ) : null}
                </button>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
}
