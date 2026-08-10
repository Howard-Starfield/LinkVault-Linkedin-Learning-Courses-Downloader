import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ensureNewspaperClippingThumbnail,
  getNewspaperClippingsPage,
  isTauriRuntime,
  type NewspaperClippingSummary
} from "./newspaper-api";
import { preserveStableClippingThumbnail } from "./clipping-thumbnail-state";

const PAGE_SIZE = 50;
const ROW_HEIGHT = 220;

export function NewspaperClippingList({
  selectedId,
  onSelect
}: {
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const generationRef = useRef(0);
  const loadingRef = useRef(new Set<string>());
  const thumbnailRef = useRef(new Set<string>());
  const thumbnailErrorRef = useRef(new Set<string>());
  const selectedIdRef = useRef(selectedId);
  const onSelectRef = useRef(onSelect);
  selectedIdRef.current = selectedId;
  onSelectRef.current = onSelect;
  const [items, setItems] = useState<Array<NewspaperClippingSummary | undefined>>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");

  const loadPage = useCallback(async (offset: number, generation: number, reset = false) => {
    const owner = `${generation}:${offset}`;
    if (loadingRef.current.has(owner)) return;
    loadingRef.current.add(owner);
    try {
      const page = await getNewspaperClippingsPage({
        query: "",
        sort: "updated_desc",
        offset,
        limit: PAGE_SIZE
      });
      if (generation !== generationRef.current) return;
      setError("");
      setTotal(page.total);
      setItems((current) => {
        const next = reset ? new Array<NewspaperClippingSummary | undefined>(page.total) : [...current];
        const previousById = new Map(
          current.flatMap((item) => item ? [[item.id, item] as const] : [])
        );
        next.length = page.total;
        page.items.forEach((item, index) => {
          next[page.offset + index] = preserveStableClippingThumbnail(item, previousById.get(item.id));
        });
        return next;
      });
      if (!selectedIdRef.current && page.items[0]) onSelectRef.current(page.items[0].id);
    } catch (cause) {
      if (generation === generationRef.current) setError(String(cause));
    } finally {
      loadingRef.current.delete(owner);
    }
  }, []);

  const refresh = useCallback(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    setItems([]);
    setTotal(0);
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
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("newspaper://clipping-invalidated", () => {
      if (!disposed) refreshLoadedPages();
    }).then((cleanup) => {
      unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshLoadedPages]);

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => items[index]?.id ?? `clipping-placeholder-${index}`,
    overscan: 4
  });
  const virtualItems = virtualizer.getVirtualItems();

  useEffect(() => {
    if (!virtualItems.length) return;
    const generation = generationRef.current;
    const offsets = new Set<number>();
    for (const row of virtualItems) {
      if (!items[row.index]) offsets.add(Math.floor(row.index / PAGE_SIZE) * PAGE_SIZE);
    }
    const last = virtualItems[virtualItems.length - 1]?.index ?? 0;
    const nextOffset = Math.floor((last + 10) / PAGE_SIZE) * PAGE_SIZE;
    if (nextOffset < total && !items[nextOffset]) offsets.add(nextOffset);
    offsets.forEach((offset) => void loadPage(offset, generation));
  }, [items, loadPage, total, virtualItems]);

  useEffect(() => {
    let disposed = false;
    for (const row of virtualItems) {
      const item = items[row.index];
      if (
        !item
        || item.thumbnailReady
        || thumbnailRef.current.has(item.id)
        || thumbnailErrorRef.current.has(item.id)
      ) continue;
      thumbnailRef.current.add(item.id);
      void ensureNewspaperClippingThumbnail(item.id)
        .then((thumbnail) => {
          if (disposed) return;
          setItems((current) => current.map((candidate) => candidate?.id === item.id
            ? {
                ...candidate,
                thumbnailReady: true,
                thumbnailUrl: thumbnail.thumbnailUrl,
                thumbnailVersion: thumbnail.thumbnailVersion
              }
            : candidate));
        })
        .catch(() => {
          thumbnailErrorRef.current.add(item.id);
          if (disposed) return;
          setItems((current) => current.map((candidate) => candidate?.id === item.id
            ? { ...candidate, thumbnailReady: false, thumbnailUrl: null, thumbnailVersion: null }
            : candidate));
        })
        .finally(() => thumbnailRef.current.delete(item.id));
    }
    return () => {
      disposed = true;
    };
  }, [items, virtualItems]);

  return (
    <aside className="clipping-list" aria-label="Saved clippings">
      <header>
        <div><span>Saved evidence</span><strong>{total} clipping{total === 1 ? "" : "s"}</strong></div>
      </header>
      <div className="clipping-list__scroll" ref={scrollRef} data-testid="newspaper-clipping-list-scroll">
        {error ? <div className="clipping-list__message" role="alert">Could not load clippings. {error}</div> : null}
        {!error && total === 0 ? <div className="clipping-list__message">Crop an area in Newspaper library to create your first note.</div> : null}
        <div className="clipping-list__virtual" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualItems.map((row) => {
            const item = items[row.index];
            return (
              <button
                aria-current={item?.id === selectedId ? "page" : undefined}
                className="clipping-list__row"
                disabled={!item}
                key={row.key}
                onClick={() => item && onSelect(item.id)}
                style={{ height: `${row.size}px`, transform: `translateY(${row.start}px)` }}
                type="button"
              >
                <span className="clipping-list__thumb">
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
                      aria-label={item?.assetState === "missing" || (item && thumbnailErrorRef.current.has(item.id))
                        ? "Clipping preview unavailable"
                        : "Preparing clipping preview"}
                      className="clipping-list__thumb-placeholder"
                      role="img"
                    />
                  )}
                </span>
                {item ? <span className="clipping-list__title"><strong>{item.title}</strong></span> : null}
              </button>
            );
          })}
        </div>
      </div>
    </aside>
  );
}
