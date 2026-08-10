import { useVirtualizer, type Range } from "@tanstack/react-virtual";
import {
  ArrowLeft,
  ChevronLeft,
  ChevronRight,
  FolderOpen,
  Maximize2,
  RotateCcw,
  Scissors,
  ZoomIn,
  ZoomOut
} from "lucide-react";
import {
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState
} from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Button, Select } from "../primitives";
import {
  createNewspaperClipping,
  getReaderManifest,
  saveReadingProgress,
  type CreateNewspaperClippingFailure,
  type NewspaperLibraryItem,
  type NewspaperReaderPage,
  type NewspaperReadingProgress
} from "./newspaper-api";
import {
  clampNewspaperReaderZoom,
  type NewspaperPageTone
} from "./newspaper-reader-preferences";
import {
  clientRectSizesMateriallyDiffer,
  estimateSourceCropSize,
  isEstimatedCropLargeEnough,
  normalizedCropRectFromClientPoints,
  type ClientPoint
} from "./newspaper-clipping-geometry";
import {
  clippingModeName,
  initialNewspaperClippingInteraction,
  newspaperClippingReducer,
  type ClippingTarget,
  type NewspaperClippingInteraction
} from "./newspaper-clipping-state";
import {
  NewspaperClippingConfirmationControls,
  NewspaperClippingSelectionOverlay
} from "./NewspaperClippingSelectionOverlay";
import { threePageRange } from "./newspaper-virtualization";

const PAGE_GAP = 2;
const PAN_DRAG_THRESHOLD = 5;

type ZoomAnchor = {
  pageIndex: number;
  image: HTMLImageElement;
  clientX: number;
  clientY: number;
  xRatio: number;
  yRatio: number;
};

type PanGesture = {
  pointerId: number;
  startClientX: number;
  startClientY: number;
  startScrollLeft: number;
  startScrollTop: number;
  previousScrollBehavior: string;
  anchor: ZoomAnchor;
  panEnabled: boolean;
  dragged: boolean;
};

type ClippingDrawingGesture = {
  pointerId: number;
  pageId: string;
  pageIndex: number;
  expectedMediaVersion: number;
  image: HTMLImageElement;
  imageRectAtStart: DOMRectReadOnly;
  start: ClientPoint;
  current: ClientPoint;
};

export type NewspaperClippingCapability = {
  enabled: boolean;
  onCreated?: (clippingId: string) => void;
};

function isEditableKeyboardTarget(target: EventTarget | null) {
  if (!(target instanceof Element)) return false;
  if (target.closest("input, textarea, select, [data-editor-root='true']")) return true;
  const editable = target.closest<HTMLElement>("[contenteditable]");
  return editable !== null && editable.getAttribute("contenteditable") !== "false";
}

function clippingFailure(value: unknown): CreateNewspaperClippingFailure | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<CreateNewspaperClippingFailure>;
  return typeof candidate.code === "string"
    && typeof candidate.safeMessage === "string"
    && typeof candidate.retryable === "boolean"
    && typeof candidate.operationId === "string"
    ? candidate as CreateNewspaperClippingFailure
    : null;
}

export function NewspaperReader({
  item,
  defaultZoom,
  clickZoom,
  pageTone,
  onPageToneChange,
  clippingCapability,
  onClose
}: {
  item: NewspaperLibraryItem;
  defaultZoom: number;
  clickZoom: number;
  pageTone: NewspaperPageTone;
  onPageToneChange: (tone: NewspaperPageTone) => void;
  clippingCapability?: NewspaperClippingCapability;
  onClose: (progress?: NewspaperReadingProgress) => void;
}) {
  const baselineZoom = clampNewspaperReaderZoom(defaultZoom);
  const scrollRef = useRef<HTMLDivElement>(null);
  const progressTimerRef = useRef<number | null>(null);
  const latestProgressRef = useRef<NewspaperReadingProgress | undefined>(undefined);
  const pendingPageRef = useRef<NewspaperReaderPage | null>(null);
  const initialScrollDoneRef = useRef(false);
  const zoomingRef = useRef(false);
  const clickZoomRestoreRef = useRef<number | null>(null);
  const panGestureRef = useRef<PanGesture | null>(null);
  const clippingDrawingRef = useRef<ClippingDrawingGesture | null>(null);
  const clippingFrameRef = useRef<number | null>(null);
  const clippingSaveRef = useRef<Promise<void> | null>(null);
  const clippingWaitTimerRef = useRef<number | null>(null);
  const [clippingInteraction, dispatchClipping] = useReducer(
    newspaperClippingReducer,
    initialNewspaperClippingInteraction
  );
  const clippingInteractionRef = useRef<NewspaperClippingInteraction>(clippingInteraction);
  const [pages, setPages] = useState<NewspaperReaderPage[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const activeIndexRef = useRef(0);
  const [zoom, setZoom] = useState(baselineZoom);
  const [containerWidth, setContainerWidth] = useState(900);
  const [loading, setLoading] = useState(true);
  const [failedImages, setFailedImages] = useState<Set<string>>(() => new Set());
  const [isClickZoomed, setIsClickZoomed] = useState(false);
  const [isPanning, setIsPanning] = useState(false);

  activeIndexRef.current = activeIndex;
  clippingInteractionRef.current = clippingInteraction;

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
  const rangeExtractor = useCallback((range: Range) => {
    const visibleIndex = Math.floor((range.startIndex + range.endIndex) / 2);
    return threePageRange(visibleIndex, pages.length);
  }, [pages.length]);

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

  const clearPanGesture = useCallback(() => {
    const gesture = panGestureRef.current;
    const element = scrollRef.current;
    if (!gesture || !element) return;
    panGestureRef.current = null;
    setIsPanning(false);
    element.style.scrollBehavior = gesture.previousScrollBehavior;
    if (element.hasPointerCapture(gesture.pointerId)) element.releasePointerCapture(gesture.pointerId);
  }, []);

  const releaseClippingPointer = useCallback(() => {
    const drawing = clippingDrawingRef.current;
    const element = scrollRef.current;
    clippingDrawingRef.current = null;
    if (clippingFrameRef.current !== null) {
      window.cancelAnimationFrame(clippingFrameRef.current);
      clippingFrameRef.current = null;
    }
    if (drawing && element?.hasPointerCapture(drawing.pointerId)) {
      element.releasePointerCapture(drawing.pointerId);
    }
  }, []);

  const cancelClipping = useCallback((announcement?: string) => {
    releaseClippingPointer();
    if (clippingWaitTimerRef.current !== null) {
      window.clearTimeout(clippingWaitTimerRef.current);
      clippingWaitTimerRef.current = null;
    }
    dispatchClipping({ type: "CANCEL", announcement });
  }, [releaseClippingPointer]);

  const enterClipMode = useCallback(() => {
    const page = pages[activeIndexRef.current];
    if (!clippingCapability?.enabled || !page || page.status !== "completed" || !page.mediaUrl) {
      toast.error("This page is not ready to clip.");
      return;
    }
    clearPanGesture();
    dispatchClipping({ type: "ENTER" });
    scrollRef.current?.focus({ preventScroll: true });
  }, [clearPanGesture, clippingCapability?.enabled, pages]);

  useEffect(() => {
    const handleBlur = () => {
      void flushProgress();
      if (clippingInteractionRef.current.type === "clip-drawing") {
        cancelClipping("Selection cancelled because the reader lost focus.");
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const interaction = clippingInteractionRef.current;
      if (event.key === "Escape") {
        event.preventDefault();
        if (interaction.type === "clip-saving") return;
        if (interaction.type !== "browse") {
          cancelClipping();
          return;
        }
        void flushProgress().then(onClose);
        return;
      }
      if (isEditableKeyboardTarget(event.target)) return;
      if (
        event.key.toLowerCase() === "c"
        && !event.ctrlKey
        && !event.metaKey
        && !event.altKey
        && !event.shiftKey
        && !event.repeat
      ) {
        if (event.target instanceof Element && event.target.closest("button, [role='dialog']")) return;
        if (!clippingCapability?.enabled) return;
        const canvas = scrollRef.current;
        if (!canvas || (document.activeElement !== canvas && !canvas.contains(document.activeElement))) return;
        event.preventDefault();
        if (interaction.type === "browse") enterClipMode();
        else if (interaction.type === "clip-selecting") cancelClipping();
        return;
      }
      if (interaction.type === "clip-drawing" || interaction.type === "clip-confirming" || interaction.type === "clip-saving") {
        if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
          event.preventDefault();
        }
        return;
      }
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        virtualizer.scrollToIndex(Math.max(0, activeIndexRef.current - 1), { align: "start" });
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        virtualizer.scrollToIndex(Math.min(pages.length - 1, activeIndexRef.current + 1), { align: "start" });
      }
    };
    window.addEventListener("blur", handleBlur);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("blur", handleBlur);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [cancelClipping, clippingCapability?.enabled, enterClipMode, flushProgress, onClose, pages.length, virtualizer]);

  useEffect(() => () => {
    releaseClippingPointer();
    if (clippingWaitTimerRef.current !== null) window.clearTimeout(clippingWaitTimerRef.current);
  }, [releaseClippingPointer]);

  const changeZoom = (nextZoom: number, anchor?: ZoomAnchor) => {
    const bounded = clampNewspaperReaderZoom(nextZoom);
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
            virtualizer.measure();
          });
        });
      });
    });
  };

  const changeZoomFromControls = (nextZoom: number) => {
    clickZoomRestoreRef.current = null;
    setIsClickZoomed(false);
    changeZoom(nextZoom);
  };

  const toggleClickZoom = (anchor: ZoomAnchor) => {
    if (isClickZoomed && clickZoomRestoreRef.current !== null) {
      const restoreZoom = clickZoomRestoreRef.current;
      clickZoomRestoreRef.current = null;
      setIsClickZoomed(false);
      changeZoom(restoreZoom, anchor);
      return;
    }
    const nextZoom = clampNewspaperReaderZoom(clickZoom);
    if (nextZoom <= zoom) return;
    clickZoomRestoreRef.current = zoom;
    setIsClickZoomed(true);
    changeZoom(nextZoom, anchor);
  };

  const panEnabled = isClickZoomed || zoom > baselineZoom + .001;

  const finishPanGesture = (element: HTMLDivElement, pointerId: number) => {
    const gesture = panGestureRef.current;
    if (!gesture || gesture.pointerId !== pointerId) return null;
    panGestureRef.current = null;
    setIsPanning(false);
    element.style.scrollBehavior = gesture.previousScrollBehavior;
    if (element.hasPointerCapture(pointerId)) element.releasePointerCapture(pointerId);
    return gesture;
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!event.isPrimary || event.button !== 0 || zoomingRef.current) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const image = target.closest<HTMLImageElement>('[data-testid="newspaper-reader-page-image"]');
    const pageElement = image?.closest<HTMLElement>(".newspaper-reader-page");
    const pageIndex = Number(pageElement?.dataset.index);
    if (!image || !Number.isInteger(pageIndex)) return;
    const rect = image.getBoundingClientRect();
    const element = event.currentTarget;
    element.focus({ preventScroll: true });
    const interaction = clippingInteractionRef.current;
    if (interaction.type === "clip-selecting") {
      const page = pages[pageIndex];
      if (
        !page
        || page.id !== pageElement?.dataset.pageId
        || page.status !== "completed"
        || page.mediaVersion <= 0
        || !image.isConnected
        || !image.complete
        || image.naturalWidth <= 0
        || image.naturalHeight <= 0
        || rect.width <= 0
        || rect.height <= 0
      ) return;
      const start = { x: event.clientX, y: event.clientY };
      clippingDrawingRef.current = {
        pointerId: event.pointerId,
        pageId: page.id,
        pageIndex,
        expectedMediaVersion: page.mediaVersion,
        image,
        imageRectAtStart: rect,
        start,
        current: start
      };
      dispatchClipping({
        type: "START",
        pointerId: event.pointerId,
        pageId: page.id,
        pageIndex,
        expectedMediaVersion: page.mediaVersion,
        rect: { x: 0, y: 0, width: 0, height: 0 },
        estimatedSize: null
      });
      element.setPointerCapture(event.pointerId);
      event.preventDefault();
      return;
    }
    if (interaction.type !== "browse") {
      event.preventDefault();
      return;
    }
    const previousScrollBehavior = element.style.scrollBehavior;
    if (panEnabled) element.style.scrollBehavior = "auto";
    panGestureRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startScrollLeft: element.scrollLeft,
      startScrollTop: element.scrollTop,
      previousScrollBehavior,
      anchor: {
        pageIndex,
        image,
        clientX: event.clientX,
        clientY: event.clientY,
        xRatio: (event.clientX - rect.left) / Math.max(1, rect.width),
        yRatio: (event.clientY - rect.top) / Math.max(1, rect.height)
      },
      panEnabled,
      dragged: false
    };
    element.setPointerCapture(event.pointerId);
    event.preventDefault();
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drawing = clippingDrawingRef.current;
    if (drawing?.pointerId === event.pointerId) {
      drawing.current = { x: event.clientX, y: event.clientY };
      if (clippingFrameRef.current === null) {
        clippingFrameRef.current = window.requestAnimationFrame(() => {
          clippingFrameRef.current = null;
          const current = clippingDrawingRef.current;
          if (!current || !current.image.isConnected) {
            cancelClipping("Selection cancelled because the page is no longer available.");
            return;
          }
          const imageRect = current.image.getBoundingClientRect();
          const rect = normalizedCropRectFromClientPoints(current.start, current.current, imageRect);
          if (!rect) return;
          const page = pages[current.pageIndex];
          dispatchClipping({
            type: "DRAW",
            rect,
            estimatedSize: estimateSourceCropSize(rect, page?.pixelWidth, page?.pixelHeight)
          });
        });
      }
      event.preventDefault();
      return;
    }
    const gesture = panGestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const deltaX = event.clientX - gesture.startClientX;
    const deltaY = event.clientY - gesture.startClientY;
    if (!gesture.dragged && Math.hypot(deltaX, deltaY) >= PAN_DRAG_THRESHOLD) {
      gesture.dragged = true;
      if (gesture.panEnabled) setIsPanning(true);
    }
    if (!gesture.dragged || !gesture.panEnabled) return;
    event.currentTarget.scrollLeft = gesture.startScrollLeft - deltaX;
    event.currentTarget.scrollTop = gesture.startScrollTop - deltaY;
    event.preventDefault();
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drawing = clippingDrawingRef.current;
    if (drawing?.pointerId === event.pointerId) {
      drawing.current = { x: event.clientX, y: event.clientY };
      const imageRect = drawing.image.getBoundingClientRect();
      const rect = normalizedCropRectFromClientPoints(drawing.start, drawing.current, imageRect);
      const page = pages[drawing.pageIndex];
      const estimatedSize = rect
        ? estimateSourceCropSize(rect, page?.pixelWidth, page?.pixelHeight)
        : null;
      releaseClippingPointer();
      event.preventDefault();
      if (!rect || !isEstimatedCropLargeEnough(estimatedSize)) {
        dispatchClipping({ type: "REJECT_SMALL" });
        return;
      }
      dispatchClipping({ type: "CONFIRM", rect, estimatedSize });
      return;
    }
    const gesture = finishPanGesture(event.currentTarget, event.pointerId);
    if (!gesture) return;
    event.preventDefault();
    if (!gesture.dragged) toggleClickZoom(gesture.anchor);
  };

  const handlePointerCancel = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (clippingDrawingRef.current?.pointerId === event.pointerId) {
      cancelClipping("Selection cancelled.");
      return;
    }
    finishPanGesture(event.currentTarget, event.pointerId);
  };

  const saveClipping = useCallback(() => {
    const interaction = clippingInteractionRef.current;
    if (interaction.type !== "clip-confirming" || interaction.requiresRedraw || clippingSaveRef.current) return;
    const operationId = interaction.operationId ?? crypto.randomUUID();
    const target: ClippingTarget = interaction;
    dispatchClipping({ type: "SAVE", operationId });
    clippingWaitTimerRef.current = window.setTimeout(() => {
      clippingWaitTimerRef.current = null;
      dispatchClipping({ type: "WAITING", operationId });
    }, 300);

    const task = (async () => {
      try {
        const response = await createNewspaperClipping({
          operationId,
          pageId: target.pageId,
          expectedMediaVersion: target.expectedMediaVersion,
          rect: target.rect
        });
        dispatchClipping({
          type: "SAVED",
          announcement: `Clipping saved from ${response.editionName}, ${response.publicationDate}, page ${response.pageNumber}.`
        });
        toast.success("Clipping saved", {
          description: `${response.editionName} · ${response.publicationDate} · ${response.pageNumber}`
        });
        clippingCapability?.onCreated?.(response.clippingId);
      } catch (error) {
        const failure = clippingFailure(error);
        if (failure?.code === "SOURCE_MEDIA_STALE") {
          try {
            const manifest = await getReaderManifest(item.jobId);
            const refreshedPage = manifest.find((page) => page.id === target.pageId);
            setPages(manifest);
            if (!refreshedPage || refreshedPage.status !== "completed") {
              cancelClipping("The source page is no longer available.");
              toast.error("Source page unavailable");
              return;
            }
            dispatchClipping({
              type: "REFRESHED",
              pageId: target.pageId,
              pageIndex: manifest.findIndex((page) => page.id === target.pageId),
              expectedMediaVersion: refreshedPage.mediaVersion,
              rect: target.rect,
              estimatedSize: estimateSourceCropSize(
                target.rect,
                refreshedPage.pixelWidth,
                refreshedPage.pixelHeight
              ),
              announcement: "The page changed. Click Redraw before saving again."
            });
            return;
          } catch {
            cancelClipping("The source page could not be refreshed.");
            toast.error("Could not refresh the source page");
            return;
          }
        }
        if (failure?.code === "CROP_TOO_SMALL") {
          dispatchClipping({
            type: "SAVE_FAILED",
            error: "Select a larger area. The saved region must be at least 32 × 32 source pixels.",
            retainOperationId: false
          });
          return;
        }
        if (failure && ["SOURCE_PAGE_NOT_FOUND", "SOURCE_PAGE_NOT_READY", "SOURCE_MEDIA_UNAVAILABLE"].includes(failure.code)) {
          cancelClipping(failure.safeMessage);
          toast.error("Source page unavailable", { description: failure.safeMessage });
          void getReaderManifest(item.jobId).then(setPages).catch(() => undefined);
          return;
        }
        if (failure && !failure.retryable) {
          cancelClipping(failure.safeMessage);
          toast.error("Clipping was not saved", { description: failure.safeMessage });
          return;
        }
        dispatchClipping({
          type: "SAVE_FAILED",
          error: failure?.safeMessage ?? "The save result is unknown. Retry uses the same operation safely.",
          retainOperationId: true
        });
      } finally {
        if (clippingWaitTimerRef.current !== null) {
          window.clearTimeout(clippingWaitTimerRef.current);
          clippingWaitTimerRef.current = null;
        }
      }
    })();
    clippingSaveRef.current = task;
    void task.finally(() => {
      if (clippingSaveRef.current === task) clippingSaveRef.current = null;
    });
  }, [cancelClipping, clippingCapability, item.jobId]);

  const clippingLocksGeometry = clippingInteraction.type === "clip-drawing"
    || clippingInteraction.type === "clip-confirming"
    || clippingInteraction.type === "clip-saving";

  useEffect(() => {
    if (!clippingLocksGeometry) return;
    const element = scrollRef.current;
    if (!element) return;
    const frozenLeft = element.scrollLeft;
    const frozenTop = element.scrollTop;
    const prevent = (event: Event) => event.preventDefault();
    const restore = () => {
      if (element.scrollLeft !== frozenLeft) element.scrollLeft = frozenLeft;
      if (element.scrollTop !== frozenTop) element.scrollTop = frozenTop;
    };
    element.addEventListener("wheel", prevent, { passive: false });
    element.addEventListener("touchmove", prevent, { passive: false });
    element.addEventListener("scroll", restore);
    return () => {
      element.removeEventListener("wheel", prevent);
      element.removeEventListener("touchmove", prevent);
      element.removeEventListener("scroll", restore);
    };
  }, [clippingLocksGeometry]);

  useEffect(() => {
    if (clippingInteraction.type !== "clip-drawing") return;
    const drawing = clippingDrawingRef.current;
    if (!drawing) return;
    const observer = new ResizeObserver(() => {
      if (clientRectSizesMateriallyDiffer(drawing.imageRectAtStart, drawing.image.getBoundingClientRect())) {
        cancelClipping("Selection cancelled because the page size changed.");
      }
    });
    observer.observe(drawing.image);
    return () => observer.disconnect();
  }, [cancelClipping, clippingInteraction.type]);

  useEffect(() => {
    if (clippingInteraction.type !== "clip-drawing") return;
    const page = pages[clippingInteraction.pageIndex];
    if (
      !page
      || page.id !== clippingInteraction.pageId
      || page.mediaVersion !== clippingInteraction.expectedMediaVersion
      || page.status !== "completed"
    ) {
      cancelClipping("Selection cancelled because the newspaper page changed.");
    }
  }, [cancelClipping, clippingInteraction, pages]);

  const activePage = pages[activeIndex];
  const clippingConfirmation = clippingInteraction.type === "clip-confirming"
    || clippingInteraction.type === "clip-saving"
    ? clippingInteraction
    : null;
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
      data-page-tone={pageTone}
      data-pan-enabled={panEnabled ? "true" : "false"}
      data-panning={isPanning ? "true" : "false"}
      data-clipping-mode={clippingModeName(clippingInteraction)}
    >
      <header className="newspaper-reader-header">
        <div className="newspaper-reader-identity">
          <Button
            className="newspaper-reader-back"
            size="xs"
            variant="ghost"
            disabled={clippingLocksGeometry}
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
            disabled={clippingLocksGeometry || activeIndex <= 0}
            onClick={() => virtualizer.scrollToIndex(Math.max(0, activeIndex - 1), { align: "start" })}
          >
            <ChevronLeft />
          </Button>
          <Select
            className="newspaper-reader-page-select"
            value={String(activeIndex)}
            onChange={(event) => virtualizer.scrollToIndex(Number(event.target.value), { align: "start" })}
            aria-label="Select newspaper page"
            disabled={clippingLocksGeometry}
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
            disabled={clippingLocksGeometry || activeIndex >= pages.length - 1}
            onClick={() => virtualizer.scrollToIndex(Math.min(pages.length - 1, activeIndex + 1), { align: "start" })}
          >
            <ChevronRight />
          </Button>
        </div>
        <div className="newspaper-reader-controls">
          <div className="newspaper-reader-zoom-controls">
            <Button size="xs" variant="ghost" aria-label="Zoom out 20 percent" disabled={clippingLocksGeometry} onClick={() => changeZoomFromControls(zoom - .2)}>
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
                disabled={clippingLocksGeometry}
                onChange={(event) => changeZoomFromControls(Number(event.target.value) / 100)}
              />
              <output>{Math.round(zoom * 100)}%</output>
            </label>
            <Button size="xs" variant="ghost" aria-label="Zoom in 20 percent" disabled={clippingLocksGeometry} onClick={() => changeZoomFromControls(zoom + .2)}>
              <ZoomIn />
            </Button>
          </div>
          <div className="newspaper-reader-control-section">
            <Button size="xs" variant="ghost" aria-label="Fit page width" disabled={clippingLocksGeometry} onClick={() => changeZoomFromControls(1)}>
              <Maximize2 /> Fit
            </Button>
          </div>
          {clippingCapability?.enabled ? (
            <div className="newspaper-reader-control-section">
              <Button
                size="xs"
                variant={clippingInteraction.type === "browse" ? "ghost" : "primary"}
                aria-label="Clip part of this newspaper page"
                aria-pressed={clippingInteraction.type !== "browse"}
                data-testid="newspaper-reader-clip"
                disabled={clippingLocksGeometry || activePage?.status !== "completed" || !activePage.mediaUrl}
                onClick={() => {
                  if (clippingInteraction.type === "browse") enterClipMode();
                  else cancelClipping();
                }}
              >
                <Scissors /> Clip
              </Button>
            </div>
          ) : null}
          <div className="newspaper-reader-control-section">
            <Select
              className="newspaper-reader-tone-select"
              value={pageTone}
              onChange={(event) => onPageToneChange(event.target.value as NewspaperPageTone)}
              aria-label="Newspaper page tone"
              disabled={clippingLocksGeometry}
            >
              <option value="original">Original</option>
              <option value="soft">Soft paper</option>
              <option value="dim">Dim paper</option>
              <option value="inverted">Inverted</option>
            </Select>
          </div>
        </div>
      </header>
      <div
        ref={scrollRef}
        className="newspaper-reader-canvas"
        data-testid="newspaper-reader-scroll"
        tabIndex={-1}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
        onLostPointerCapture={handlePointerCancel}
      >
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
            const clippingForPage = (
              clippingInteraction.type === "clip-drawing"
              || clippingInteraction.type === "clip-confirming"
              || clippingInteraction.type === "clip-saving"
            ) && clippingInteraction.pageId === page.id
              ? clippingInteraction
              : null;
            return (
              <article
                key={page.id}
                className="newspaper-reader-page"
                data-index={virtualItem.index}
                data-page-id={page.id}
                data-page-index={virtualItem.index}
                data-media-version={page.mediaVersion}
                data-source-width={page.pixelWidth ?? undefined}
                data-source-height={page.pixelHeight ?? undefined}
                style={{
                  width: `${pageWidth}px`,
                  minHeight: `${Math.max(0, virtualItem.size)}px`,
                  transform: `translate(-50%, ${virtualItem.start}px)`
                }}
              >
                {page.status === "completed" && page.mediaUrl && !failed ? (
                  <div className="newspaper-reader-page-media">
                    <img
                      src={page.mediaUrl}
                      alt={`Page ${page.pageNumber}`}
                      draggable={false}
                      loading="eager"
                      decoding="async"
                      width={page.pixelWidth ?? 2500}
                      height={page.pixelHeight ?? 4384}
                      data-testid="newspaper-reader-page-image"
                      data-click-zoomed={isClickZoomed ? "true" : undefined}
                      onError={() => setFailedImages((current) => new Set(current).add(page.id))}
                    />
                    {clippingForPage && clippingForPage.rect.width > 0 && clippingForPage.rect.height > 0 ? (
                      <NewspaperClippingSelectionOverlay
                        rect={clippingForPage.rect}
                        estimatedSize={clippingForPage.estimatedSize}
                      />
                    ) : null}
                  </div>
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
      {clippingConfirmation ? (
        <NewspaperClippingConfirmationControls
          saving={clippingConfirmation.type === "clip-saving"}
          waiting={clippingConfirmation.type === "clip-saving" && clippingConfirmation.waiting}
          saveDisabled={clippingConfirmation.type === "clip-confirming" && Boolean(clippingConfirmation.requiresRedraw)}
          error={clippingConfirmation.type === "clip-confirming" ? clippingConfirmation.error : undefined}
          onSave={saveClipping}
          onRedraw={() => dispatchClipping({ type: "REDRAW" })}
          onCancel={() => cancelClipping()}
        />
      ) : null}
      <div className="sr-only" aria-live="polite" aria-atomic="true">
        {"announcement" in clippingInteraction ? clippingInteraction.announcement : ""}
      </div>
      <div className="newspaper-reader-tone-overlay" aria-hidden="true" />
    </section>,
    document.body
  );
}
