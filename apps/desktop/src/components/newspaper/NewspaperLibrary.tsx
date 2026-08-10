import { useVirtualizer } from "@tanstack/react-virtual";
import { FolderOpen, RotateCcw, Search } from "lucide-react";
import { type CSSProperties, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Button, Input, Select, StatusBadge } from "../primitives";
import {
  ensureThumbnail,
  getLibraryItem,
  getLibraryPage,
  isTauriRuntime,
  type NewspaperEditionKind,
  type NewspaperLibraryItem,
  type NewspaperLibraryStatus
} from "./newspaper-api";
import type { NewspaperReaderSourceTarget } from "./newspaper-navigation";
import { NewspaperReader, type NewspaperClippingCapability } from "./NewspaperReader";
import {
  NEWSPAPER_READER_PREFERENCES_EVENT,
  readNewspaperReaderPreferences,
  writeNewspaperReaderPreferences,
  type NewspaperReaderPreferences
} from "./newspaper-reader-preferences";
import { visibleVirtualIndexes } from "./newspaper-virtualization";

const PAGE_SIZE = 50;
const ROW_HEIGHT = 112;
export function NewspaperLibrary({
  clippingCapability,
  readerTarget,
  onReaderTargetConsumed,
  onReturnClipping
}: {
  clippingCapability?: NewspaperClippingCapability;
  readerTarget?: NewspaperReaderSourceTarget | null;
  onReaderTargetConsumed?: (generation: number) => void;
  onReturnClipping?: (clippingId: string) => void;
} = {}) {
  const browserHarnessCapability = typeof window !== "undefined"
    && window.location.hostname === "127.0.0.1"
    && (window as Window & { __NEWSPAPER_CLIPPING_HARNESS__?: boolean }).__NEWSPAPER_CLIPPING_HARNESS__
    ? {
        enabled: true,
        onCreated: (clippingId: string) => window.dispatchEvent(new CustomEvent(
          "linkvault:newspaper-clipping-created",
          { detail: { clippingId } }
        ))
      }
    : undefined;
  const resolvedClippingCapability = browserHarnessCapability ?? clippingCapability;
  const scrollRef = useRef<HTMLDivElement>(null);
  const requestGenerationRef = useRef(0);
  const loadingOffsetsRef = useRef<Set<number>>(new Set());
  const thumbnailRequestsRef = useRef<Set<string>>(new Set());
  const savedScrollTopRef = useRef(0);
  const [readerPreferences, setReaderPreferences] = useState(readNewspaperReaderPreferences);
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [kind, setKind] = useState<NewspaperEditionKind>("all");
  const [status, setStatus] = useState<NewspaperLibraryStatus>("all");
  const [items, setItems] = useState<Array<NewspaperLibraryItem | undefined>>([]);
  const [total, setTotal] = useState(0);
  const [readerItem, setReaderItem] = useState<NewspaperLibraryItem | null>(null);
  const [activeReaderTarget, setActiveReaderTarget] = useState<NewspaperReaderSourceTarget | null>(null);
  const [thumbnailRetryTick, setThumbnailRetryTick] = useState(0);

  useEffect(() => {
    if (!readerTarget) return;
    const generation = readerTarget.generation;
    let stale = false;
    void getLibraryItem(readerTarget.jobId).then((item) => {
      if (stale) return;
      savedScrollTopRef.current = scrollRef.current?.scrollTop ?? 0;
      setActiveReaderTarget(readerTarget);
      setReaderItem(item);
      onReaderTargetConsumed?.(generation);
    }, () => {
      if (stale) return;
      toast.error("Original edition is unavailable", {
        description: "Your clipping and note are still saved."
      });
      onReaderTargetConsumed?.(generation);
    });
    return () => {
      stale = true;
    };
  }, [onReaderTargetConsumed, readerTarget]);

  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedQuery(query.trim()), 200);
    return () => window.clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    const handlePreferences = (event: Event) => {
      const detail = (event as CustomEvent<NewspaperReaderPreferences>).detail;
      setReaderPreferences(detail ?? readNewspaperReaderPreferences());
    };
    window.addEventListener(NEWSPAPER_READER_PREFERENCES_EVENT, handlePreferences);
    return () => window.removeEventListener(NEWSPAPER_READER_PREFERENCES_EVENT, handlePreferences);
  }, []);

  const loadPage = async (offset: number, generation: number, reset = false) => {
    if (!isTauriRuntime() || loadingOffsetsRef.current.has(offset)) return;
    loadingOffsetsRef.current.add(offset);
    try {
      const page = await getLibraryPage({
        query: debouncedQuery,
        kind,
        status,
        offset,
        limit: PAGE_SIZE
      });
      if (generation !== requestGenerationRef.current) return;
      setTotal(page.total);
      setItems((current) => {
        const next = reset ? new Array<NewspaperLibraryItem | undefined>(page.total) : [...current];
        next.length = page.total;
        page.items.forEach((item, index) => {
          next[page.offset + index] = item;
        });
        return next;
      });
    } catch (error) {
      if (generation === requestGenerationRef.current) {
        toast.error("Could not load newspaper library", { description: String(error) });
      }
    } finally {
      loadingOffsetsRef.current.delete(offset);
    }
  };

  useEffect(() => {
    const generation = requestGenerationRef.current + 1;
    requestGenerationRef.current = generation;
    loadingOffsetsRef.current.clear();
    setItems([]);
    setTotal(0);
    void loadPage(0, generation, true);
  }, [debouncedQuery, kind, status]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ revision: number; jobIds: string[] }>("newspaper://library-invalidated", () => {
      if (disposed) return;
      const generation = requestGenerationRef.current + 1;
      requestGenerationRef.current = generation;
      const top = scrollRef.current?.scrollTop ?? 0;
      void loadPage(0, generation, true).finally(() => {
        requestAnimationFrame(() => {
          if (scrollRef.current) scrollRef.current.scrollTop = top;
        });
      });
    }).then((cleanup) => {
      unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [debouncedQuery, kind, status]);

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => items[index]?.jobId ?? `newspaper-placeholder-${index}`,
    overscan: 4
  });
  const virtualItems = virtualizer.getVirtualItems();
  const visibleIndexes = visibleVirtualIndexes(
    virtualItems,
    virtualizer.scrollOffset ?? 0,
    virtualizer.scrollRect?.height ?? 0
  );

  useEffect(() => {
    if (!virtualItems.length) return;
    const generation = requestGenerationRef.current;
    const offsets = new Set(
      virtualItems
        .filter((virtualItem) => !items[virtualItem.index])
        .map((virtualItem) => Math.floor(virtualItem.index / PAGE_SIZE) * PAGE_SIZE)
    );
    const last = virtualItems[virtualItems.length - 1].index;
    const prefetchOffset = Math.floor((last + 10) / PAGE_SIZE) * PAGE_SIZE;
    if (prefetchOffset < total && !items[prefetchOffset]) {
      offsets.add(prefetchOffset);
    }
    offsets.forEach((offset) => void loadPage(offset, generation));
  }, [items, total, virtualItems]);

  useEffect(() => {
    let disposed = false;
    const requestThumbnail = (item: NewspaperLibraryItem) => {
      if (disposed || thumbnailRequestsRef.current.has(item.jobId)) return;
      thumbnailRequestsRef.current.add(item.jobId);
      void ensureThumbnail(item.jobId)
        .then((result) => {
          if (disposed) return;
          if (result.status === "busy") {
            window.setTimeout(() => {
              thumbnailRequestsRef.current.delete(item.jobId);
              if (!disposed) setThumbnailRetryTick((current) => current + 1);
            }, result.retryAfterMs);
            return;
          }
          setItems((current) => current.map((candidate) => (
            candidate?.jobId === item.jobId
              ? {
                  ...candidate,
                  thumbnailReady: true,
                  thumbnailUrl: result.thumbnailUrl,
                  thumbnailVersion: result.thumbnailVersion
                }
              : candidate
          )));
          thumbnailRequestsRef.current.delete(item.jobId);
        })
        .catch(() => {
          thumbnailRequestsRef.current.delete(item.jobId);
        });
    };
    for (const virtualItem of virtualItems) {
      if (!visibleIndexes.has(virtualItem.index)) continue;
      const item = items[virtualItem.index];
      if (!item || item.thumbnailReady) continue;
      requestThumbnail(item);
    }
    return () => {
      disposed = true;
    };
  }, [items, thumbnailRetryTick, virtualItems]);

  const openedItem = useMemo(
    () => readerItem ? items.find((item) => item?.jobId === readerItem.jobId) ?? readerItem : null,
    [items, readerItem]
  );
  if (openedItem) {
    return (
      <NewspaperReader
        item={openedItem}
        defaultZoom={readerPreferences.defaultZoom}
        clickZoom={readerPreferences.clickZoom}
        pageTone={readerPreferences.pageTone}
        clippingCapability={resolvedClippingCapability}
        sourceTarget={activeReaderTarget}
        onPageToneChange={(pageTone) => {
          const next = { ...readerPreferences, pageTone };
          setReaderPreferences(next);
          writeNewspaperReaderPreferences(next);
        }}
        onClose={(progress) => {
          if (progress) {
            setItems((current) => current.map((candidate) => (
              candidate?.jobId === progress.jobId
                ? {
                    ...candidate,
                    lastPageId: progress.lastPageId,
                    lastPageIndex: progress.lastPageIndex,
                    furthestPageIndex: progress.furthestPageIndex,
                    readPageCount: progress.readPageCount,
                    readingUpdatedAt: progress.updatedAt
                  }
                : candidate
            )));
          }
          const returnClippingId = activeReaderTarget?.returnClippingId;
          setReaderItem(null);
          setActiveReaderTarget(null);
          if (returnClippingId) {
            onReturnClipping?.(returnClippingId);
            return;
          }
          requestAnimationFrame(() => {
            if (scrollRef.current) scrollRef.current.scrollTop = savedScrollTopRef.current;
          });
        }}
      />
    );
  }

  return (
    <section className="newspaper-library" aria-label="Newspaper library">
      <div className="newspaper-library-toolbar">
        <label className="newspaper-search">
          <Search aria-hidden="true" />
          <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search editions or dates" aria-label="Search newspaper library" />
        </label>
        <Select value={kind} onChange={(event) => setKind(event.target.value as NewspaperEditionKind)} aria-label="Filter newspaper kind">
          <option value="all">All publications</option>
          <option value="daily">Daily</option>
          <option value="weekly">Weekly</option>
          <option value="special">Special</option>
        </Select>
        <Select value={status} onChange={(event) => setStatus(event.target.value as NewspaperLibraryStatus)} aria-label="Filter newspaper status">
          <option value="all">All statuses</option>
          <option value="completed">Completed</option>
          <option value="partial">Partial</option>
          <option value="optimizing">Optimizing</option>
        </Select>
      </div>
      <div ref={scrollRef} className="newspaper-library-list" data-testid="newspaper-library-scroll">
        {total === 0 ? <div className="newspaper-empty">Downloaded editions will appear here.</div> : null}
        <div className="newspaper-library-virtual" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualItems.map((virtualItem) => {
            const item = items[virtualItem.index];
            if (!item) {
              return (
                <div
                  className="newspaper-library-row newspaper-library-row-skeleton"
                  key={virtualItem.key}
                  style={{ height: `${virtualItem.size}px`, transform: `translateY(${virtualItem.start}px)` }}
                />
              );
            }
            const readPageCount = Math.min(item.pageCount, item.readPageCount);
            const progressPercent = Math.min(100, Math.round((readPageCount / Math.max(1, item.pageCount)) * 100));
            const progressLabel = item.lastPageIndex !== null && item.lastPageIndex !== undefined
              ? `${readPageCount} of ${item.pageCount} pages viewed · resumes at page ${item.lastPageIndex + 1}`
              : "Not started";
            return (
              <article
                className="newspaper-library-row"
                data-job-id={item.jobId}
                key={virtualItem.key}
                style={{ height: `${virtualItem.size}px`, transform: `translateY(${virtualItem.start}px)` }}
              >
                <button
                  type="button"
                  className="newspaper-library-open"
                  aria-label={`Read ${item.editionName} from ${item.publicationDate}. ${progressLabel}`}
                  onClick={() => {
                    savedScrollTopRef.current = scrollRef.current?.scrollTop ?? 0;
                    setReaderItem(item);
                  }}
                />
                <div className="newspaper-preview">
                  {visibleIndexes.has(virtualItem.index) && item.thumbnailReady && item.thumbnailUrl ? (
                    <img
                      src={item.thumbnailUrl}
                      alt={`${item.editionName} front page preview`}
                      width={420}
                      height={176}
                      loading="lazy"
                      decoding="async"
                    />
                  ) : <span>{item.editionCode}</span>}
                </div>
                <div className="newspaper-library-copy">
                  <strong>{item.editionName}</strong>
                  <span>{item.editionCode} · {item.publicationDate} · {item.completedCount}/{item.pageCount} pages</span>
                  {item.warning ? <small>{item.warning}</small> : null}
                </div>
                <StatusBadge tone={item.status === "completed" ? "success" : item.status === "optimizing" ? "primary" : "danger"}>
                  {item.status === "optimizing" ? "optimizing images" : item.status}
                </StatusBadge>
                <div
                  className="newspaper-reading-progress"
                  role="progressbar"
                  aria-label={`Reading progress for ${item.editionName}`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={progressPercent}
                  title={progressLabel}
                  style={{ "--reading-progress": `${progressPercent}%` } as CSSProperties}
                >
                  <span>{progressPercent}%</span>
                </div>
                <div className="newspaper-row-actions">
                  {item.status === "partial" ? (
                    <Button size="xs" variant="ghost" onClick={() => void invoke("retry_newspaper_job", { jobId: item.jobId })}>
                      <RotateCcw /> Retry missing
                    </Button>
                  ) : null}
                  <Button size="xs" variant="ghost" onClick={() => void invoke("open_newspaper_download_folder", { path: item.outputDir })}>
                    <FolderOpen /> Folder
                  </Button>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
