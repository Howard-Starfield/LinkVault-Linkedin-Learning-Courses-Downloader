import { useVirtualizer, type Range } from "@tanstack/react-virtual";
import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  FolderOpen,
  Maximize2,
  RotateCcw,
  ZoomIn,
  ZoomOut
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Button, Select } from "../primitives";
import {
  getReaderManifest,
  saveReadingProgress,
  type NewspaperLibraryItem,
  type NewspaperReaderPage,
  type NewspaperReadingProgress
} from "./newspaper-api";
import { threePageRange } from "./newspaper-virtualization";

const PAGE_GAP = 2;
const CLICK_ZOOM = 1.6;

type ZoomAnchor = {
  pageIndex: number;
  image: HTMLImageElement;
  clientX: number;
  clientY: number;
  xRatio: number;
  yRatio: number;
};

export function NewspaperReader({
  item,
  onClose
}: {
  item: NewspaperLibraryItem;
  onClose: (progress?: NewspaperReadingProgress) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const progressTimerRef = useRef<number | null>(null);
  const latestProgressRef = useRef<NewspaperReadingProgress | undefined>(undefined);
  const pendingPageRef = useRef<NewspaperReaderPage | null>(null);
  const initialScrollDoneRef = useRef(false);
  const zoomingRef = useRef(false);
  const [pages, setPages] = useState<NewspaperReaderPage[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const activeIndexRef = useRef(0);
  const [zoom, setZoom] = useState(1);
  const [containerWidth, setContainerWidth] = useState(900);
  const [loading, setLoading] = useState(true);
  const [failedImages, setFailedImages] = useState<Set<string>>(() => new Set());

  activeIndexRef.current = activeIndex;

  useEffect(() => {
    let stale = false;
    setLoading(true);
    void getReaderManifest(item.jobId)
      .then((manifest) => {
        if (stale) return;
        setPages(manifest);
        const savedIndex = item.lastPageId
          ? manifest.findIndex((page) => page.id === item.lastPageId && page.status === "completed")
          : -1;
        const firstCompleted = manifest.findIndex((page) => page.status === "completed");
        const nextIndex = savedIndex >= 0 ? savedIndex : Math.max(0, firstCompleted);
        setActiveIndex(nextIndex);
        activeIndexRef.current = nextIndex;
      })
      .catch((error) => {
        if (!stale) toast.error("Could not open newspaper", { description: String(error) });
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [item.jobId, item.lastPageId]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      setContainerWidth(Math.max(320, entry.contentRect.width));
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const pageWidth = Math.max(280, containerWidth * zoom);
  const estimatePageSize = useCallback((index: number) => {
    const page = pages[index];
    const width = page?.pixelWidth ?? 2500;
    const height = page?.pixelHeight ?? 4384;
    return pageWidth * (height / Math.max(1, width));
  }, [pageWidth, pages]);
  const rangeExtractor = useCallback((_range: Range) => (
    threePageRange(activeIndexRef.current, pages.length)
  ), [pages.length]);

  const virtualizer = useVirtualizer({
    count: pages.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: estimatePageSize,
    getItemKey: (index) => pages[index]?.id ?? index,
    overscan: 0,
    gap: PAGE_GAP,
    rangeExtractor,
    onChange: (instance) => {
      if (zoomingRef.current) return;
      const measurements = instance.measurementsCache;
      if (measurements.length === 0) return;
      const center = (instance.scrollOffset ?? 0) + (instance.scrollRect?.height ?? 0) / 2;
      let nextIndex = measurements.findIndex((measurement) =>
        center >= measurement.start && center < measurement.end
      );
      if (nextIndex < 0) {
        nextIndex = center < measurements[0].start ? 0 : measurements.length - 1;
      }
      if (nextIndex !== activeIndexRef.current) {
        activeIndexRef.current = nextIndex;
        setActiveIndex(nextIndex);
      }
    }
  });

  useEffect(() => {
    virtualizer.measure();
  }, [estimatePageSize, virtualizer]);

  useEffect(() => {
    if (loading || pages.length === 0 || initialScrollDoneRef.current) return;
    initialScrollDoneRef.current = true;
    requestAnimationFrame(() => virtualizer.scrollToIndex(activeIndexRef.current, {
      align: "start",
      behavior: "auto"
    }));
  }, [loading, pages.length, virtualizer]);

  const flushProgress = useCallback(async () => {
    if (progressTimerRef.current !== null) {
      window.clearTimeout(progressTimerRef.current);
      progressTimerRef.current = null;
    }
    const page = pendingPageRef.current;
    if (!page || page.status !== "completed") return latestProgressRef.current;
    pendingPageRef.current = null;
    try {
      const saved = await saveReadingProgress(item.jobId, page.id);
      latestProgressRef.current = saved;
      return saved;
    } catch (error) {
      await new Promise((resolve) => window.setTimeout(resolve, 250));
      try {
        const saved = await saveReadingProgress(item.jobId, page.id);
        latestProgressRef.current = saved;
        return saved;
      } catch (retryError) {
        toast.error("Could not save reading progress", { description: String(retryError) });
        return latestProgressRef.current;
      }
    }
  }, [item.jobId]);

  useEffect(() => {
    const page = pages[activeIndex];
    if (!page || page.status !== "completed") return;
    pendingPageRef.current = page;
    if (progressTimerRef.current !== null) window.clearTimeout(progressTimerRef.current);
    progressTimerRef.current = window.setTimeout(() => {
      progressTimerRef.current = null;
      void flushProgress();
    }, 400);
    return () => {
      if (progressTimerRef.current !== null) {
        window.clearTimeout(progressTimerRef.current);
        progressTimerRef.current = null;
      }
    };
  }, [activeIndex, flushProgress, pages]);

  useEffect(() => {
    const handleBlur = () => void flushProgress();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        virtualizer.scrollToIndex(Math.max(0, activeIndexRef.current - 1), { align: "start" });
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        virtualizer.scrollToIndex(Math.min(pages.length - 1, activeIndexRef.current + 1), { align: "start" });
      } else if (event.key === "Escape") {
        event.preventDefault();
        void flushProgress().then(onClose);
      }
    };
    window.addEventListener("blur", handleBlur);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("blur", handleBlur);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [flushProgress, onClose, pages.length, virtualizer]);

  const changeZoom = (nextZoom: number, anchor?: ZoomAnchor) => {
    const bounded = Math.max(.5, Math.min(3, nextZoom));
    const element = scrollRef.current;
    const targetIndex = anchor?.pageIndex ?? activeIndexRef.current;
    const previousScrollBehavior = element?.style.scrollBehavior ?? "";
    if (element) element.style.scrollBehavior = "auto";
    activeIndexRef.current = targetIndex;
    setActiveIndex(targetIndex);
    zoomingRef.current = true;
    setZoom(bounded);
    requestAnimationFrame(() => {
      virtualizer.measure();
      requestAnimationFrame(() => {
        if (element && anchor?.image.isConnected) {
          const nextRect = anchor.image.getBoundingClientRect();
          element.scrollLeft += nextRect.left + nextRect.width * anchor.xRatio - anchor.clientX;
          element.scrollTop += nextRect.top + nextRect.height * anchor.yRatio - anchor.clientY;
        } else {
          if (element) element.scrollLeft = Math.max(0, (element.scrollWidth - element.clientWidth) / 2);
          virtualizer.scrollToIndex(targetIndex, { align: "center", behavior: "auto" });
        }
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            zoomingRef.current = false;
            if (element) element.style.scrollBehavior = previousScrollBehavior;
          });
        });
      });
    });
  };

  const activePage = pages[activeIndex];
  const virtualItems = virtualizer.getVirtualItems();
  const maxMountedImages = useMemo(
    () => virtualItems.filter((virtualItem) => pages[virtualItem.index]?.mediaUrl).length,
    [pages, virtualItems]
  );

  return createPortal(
    <section
      className="newspaper-reader"
      aria-label={`${item.editionName} reader`}
      data-mounted-page-images={maxMountedImages}
    >
      <header className="newspaper-reader-header">
        <div className="newspaper-reader-identity">
          <Button
            className="newspaper-reader-back"
            size="xs"
            variant="ghost"
            onClick={() => void flushProgress().then(onClose)}
          >
            <ArrowLeft /> Back to library
          </Button>
          <div>
            <strong>{item.editionName}</strong>
            <span>{item.publicationDate} · {activePage?.pageNumber ?? "Loading"}</span>
          </div>
        </div>
        <div className="newspaper-reader-pagination" aria-label="Newspaper page navigation">
          <Button
            size="xs"
            variant="ghost"
            aria-label="Previous page"
            disabled={activeIndex <= 0}
            onClick={() => virtualizer.scrollToIndex(Math.max(0, activeIndex - 1), { align: "start" })}
          >
            <ChevronLeft />
          </Button>
          <Select
            className="newspaper-reader-page-select"
            value={String(activeIndex)}
            onChange={(event) => virtualizer.scrollToIndex(Number(event.target.value), { align: "start" })}
            aria-label="Select newspaper page"
          >
            {pages.map((page, index) => (
              <option key={page.id} value={index}>{page.pageNumber}</option>
            ))}
          </Select>
          <span aria-live="polite">{pages.length ? activeIndex + 1 : 0} / {pages.length}</span>
          <Button
            size="xs"
            variant="ghost"
            aria-label="Next page"
            disabled={activeIndex >= pages.length - 1}
            onClick={() => virtualizer.scrollToIndex(Math.min(pages.length - 1, activeIndex + 1), { align: "start" })}
          >
            <ChevronRight />
          </Button>
        </div>
        <div className="newspaper-reader-controls">
          <Button size="xs" variant="ghost" aria-label="Zoom out 20 percent" onClick={() => changeZoom(zoom - .2)}>
            <ZoomOut />
          </Button>
          <label className="newspaper-reader-zoom">
            <span className="sr-only">Reader zoom</span>
            <input
              type="range"
              min="50"
              max="300"
              step="10"
              value={Math.round(zoom * 100)}
              onChange={(event) => changeZoom(Number(event.target.value) / 100)}
            />
            <output>{Math.round(zoom * 100)}%</output>
          </label>
          <Button size="xs" variant="ghost" aria-label="Zoom in 20 percent" onClick={() => changeZoom(zoom + .2)}>
            <ZoomIn />
          </Button>
          <Button size="xs" variant="ghost" aria-label="Fit page width" onClick={() => changeZoom(1)}>
            <Maximize2 /> Fit
          </Button>
        </div>
      </header>
      <div ref={scrollRef} className="newspaper-reader-canvas" data-testid="newspaper-reader-scroll">
        {loading ? <div className="newspaper-reader-loading">Loading newspaper…</div> : null}
        <div
          className="newspaper-reader-virtual"
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            minWidth: `${pageWidth}px`
          }}
        >
          {virtualItems.map((virtualItem) => {
            const page = pages[virtualItem.index];
            if (!page) return null;
            const failed = failedImages.has(page.id);
            return (
              <article
                key={page.id}
                className="newspaper-reader-page"
                data-index={virtualItem.index}
                data-page-id={page.id}
                style={{
                  width: `${pageWidth}px`,
                  minHeight: `${Math.max(0, virtualItem.size)}px`,
                  transform: `translate(-50%, ${virtualItem.start}px)`
                }}
              >
                {page.status === "completed" && page.mediaUrl && !failed ? (
                  <img
                    src={page.mediaUrl}
                    alt={`Page ${page.pageNumber}`}
                    draggable={false}
                    loading="eager"
                    decoding="async"
                    width={page.pixelWidth ?? 2500}
                    height={page.pixelHeight ?? 4384}
                    data-testid="newspaper-reader-page-image"
                    data-click-zoomed={zoom > 1 ? "true" : undefined}
                    onClick={(event) => {
                      const rect = event.currentTarget.getBoundingClientRect();
                      changeZoom(zoom > 1 ? 1 : CLICK_ZOOM, {
                        pageIndex: virtualItem.index,
                        image: event.currentTarget,
                        clientX: event.clientX,
                        clientY: event.clientY,
                        xRatio: (event.clientX - rect.left) / Math.max(1, rect.width),
                        yRatio: (event.clientY - rect.top) / Math.max(1, rect.height)
                      });
                    }}
                    onError={() => setFailedImages((current) => new Set(current).add(page.id))}
                  />
                ) : (
                  <div className="newspaper-reader-page-error" role="status">
                    <strong>Page {page.pageNumber} is unavailable.</strong>
                    <span>{page.error ?? "The local image could not be loaded."}</span>
                    <div>
                      <Button size="xs" variant="ghost" onClick={() => {
                        setFailedImages((current) => {
                          const next = new Set(current);
                          next.delete(page.id);
                          return next;
                        });
                      }}><RotateCcw /> Retry</Button>
                      <Button size="xs" variant="ghost" onClick={() => void invoke("open_newspaper_download_folder", { path: item.outputDir })}>
                        <FolderOpen /> Folder
                      </Button>
                    </div>
                  </div>
                )}
              </article>
            );
          })}
        </div>
      </div>
    </section>,
    document.body
  );
}
