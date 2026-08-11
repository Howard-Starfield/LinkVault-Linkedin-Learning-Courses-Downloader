import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from "react";

const MIN_PREVIEW_ZOOM = 2;
const MAX_PREVIEW_ZOOM = 4;
const DRAG_THRESHOLD_PX = 4;
type Point = { x: number; y: number };
type PanGesture = {
  pointerId: number;
  startClient: Point;
  startPan: Point;
  dragged: boolean;
};

function clamp(value: number, limit: number) {
  return Math.max(-limit, Math.min(limit, value));
}

export function useClippingImagePreview() {
  const [open, setOpen] = useState(false);
  const [scale, setScale] = useState(1);
  const [pan, setPan] = useState<Point>({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const gestureRef = useRef<PanGesture | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  const resetView = useCallback(() => {
    gestureRef.current = null;
    setDragging(false);
    setScale(1);
    setPan({ x: 0, y: 0 });
  }, []);

  const openPreview = useCallback(() => {
    resetView();
    setOpen(true);
  }, [resetView]);

  const closePreview = useCallback(() => {
    setOpen(false);
    resetView();
    requestAnimationFrame(() => triggerRef.current?.focus());
  }, [resetView]);

  const panLimits = useCallback((nextScale: number) => {
    const image = imageRef.current;
    const viewport = viewportRef.current;
    if (!image || !viewport) return { x: 0, y: 0 };
    return {
      x: Math.max(0, (image.offsetWidth * nextScale - viewport.clientWidth) / 2),
      y: Math.max(0, (image.offsetHeight * nextScale - viewport.clientHeight) / 2)
    };
  }, []);

  const previewZoom = useCallback(() => {
    const image = imageRef.current;
    const viewport = viewportRef.current;
    if (!image || !viewport || image.offsetWidth === 0 || image.offsetHeight === 0) return MIN_PREVIEW_ZOOM;
    const fillViewport = Math.max(
      viewport.clientWidth / image.offsetWidth,
      viewport.clientHeight / image.offsetHeight
    );
    return Math.min(MAX_PREVIEW_ZOOM, Math.max(MIN_PREVIEW_ZOOM, fillViewport * 1.2));
  }, []);

  const toggleZoomAt = useCallback((client: Point) => {
    if (scale > 1) {
      setScale(1);
      setPan({ x: 0, y: 0 });
      return;
    }
    const image = imageRef.current;
    if (!image) return;
    const rect = image.getBoundingClientRect();
    const nextScale = previewZoom();
    const limits = panLimits(nextScale);
    setScale(nextScale);
    setPan({
      x: clamp(-(client.x - (rect.left + rect.width / 2)), limits.x),
      y: clamp(-(client.y - (rect.top + rect.height / 2)), limits.y)
    });
  }, [panLimits, previewZoom, scale]);

  const onImagePointerDown = useCallback((event: ReactPointerEvent<HTMLImageElement>) => {
    if (!event.isPrimary || event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    gestureRef.current = {
      pointerId: event.pointerId,
      startClient: { x: event.clientX, y: event.clientY },
      startPan: pan,
      dragged: false
    };
  }, [pan]);

  const onImagePointerMove = useCallback((event: ReactPointerEvent<HTMLImageElement>) => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const delta = {
      x: event.clientX - gesture.startClient.x,
      y: event.clientY - gesture.startClient.y
    };
    if (!gesture.dragged && Math.hypot(delta.x, delta.y) >= DRAG_THRESHOLD_PX) {
      gesture.dragged = true;
      setDragging(true);
    }
    if (!gesture.dragged || scale <= 1) return;
    const limits = panLimits(scale);
    setPan({
      x: clamp(gesture.startPan.x + delta.x, limits.x),
      y: clamp(gesture.startPan.y + delta.y, limits.y)
    });
  }, [panLimits, scale]);

  const finishImagePointer = useCallback((event: ReactPointerEvent<HTMLImageElement>) => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    gestureRef.current = null;
    setDragging(false);
    if (!gesture.dragged) toggleZoomAt({ x: event.clientX, y: event.clientY });
  }, [toggleZoomAt]);

  const cancelImagePointer = useCallback((event: ReactPointerEvent<HTMLImageElement>) => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    gestureRef.current = null;
    setDragging(false);
  }, []);

  const onImageKeyDown = useCallback((event: ReactKeyboardEvent<HTMLImageElement>) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    toggleZoomAt({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
  }, [toggleZoomAt]);

  const imageStyle = {
    "--clipping-preview-scale": scale,
    "--clipping-preview-x": `${pan.x}px`,
    "--clipping-preview-y": `${pan.y}px`
  } as CSSProperties;
  return {
    closePreview,
    dialogRef,
    dragging,
    imageRef,
    imageStyle,
    onImageKeyDown,
    onImagePointerDown,
    onImagePointerMove,
    onImagePointerUp: finishImagePointer,
    onImagePointerCancel: cancelImagePointer,
    open,
    openPreview,
    scale,
    triggerRef,
    viewportRef
  };
}
