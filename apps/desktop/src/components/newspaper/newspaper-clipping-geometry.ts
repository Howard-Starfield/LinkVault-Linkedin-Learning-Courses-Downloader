import type { NormalizedCropRect } from "./newspaper-api";

export type ClientPoint = {
  x: number;
  y: number;
};

export type ClientRectLike = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export type EstimatedCropSize = {
  width: number;
  height: number;
};

const SIZE_EPSILON = 0.5;

function finite(value: number) {
  return Number.isFinite(value);
}

export function isUsableClientRect(rect: ClientRectLike) {
  return finite(rect.left)
    && finite(rect.top)
    && finite(rect.width)
    && finite(rect.height)
    && rect.width > 0
    && rect.height > 0;
}

export function clampClientPoint(point: ClientPoint, rect: ClientRectLike): ClientPoint | null {
  if (!finite(point.x) || !finite(point.y) || !isUsableClientRect(rect)) return null;
  return {
    x: Math.min(rect.left + rect.width, Math.max(rect.left, point.x)),
    y: Math.min(rect.top + rect.height, Math.max(rect.top, point.y))
  };
}

export function normalizedCropRectFromClientPoints(
  start: ClientPoint,
  current: ClientPoint,
  rect: ClientRectLike
): NormalizedCropRect | null {
  const boundedStart = clampClientPoint(start, rect);
  const boundedCurrent = clampClientPoint(current, rect);
  if (!boundedStart || !boundedCurrent) return null;
  const left = Math.min(boundedStart.x, boundedCurrent.x);
  const top = Math.min(boundedStart.y, boundedCurrent.y);
  const right = Math.max(boundedStart.x, boundedCurrent.x);
  const bottom = Math.max(boundedStart.y, boundedCurrent.y);
  if (right <= left || bottom <= top) return null;
  const normalized = {
    x: (left - rect.left) / rect.width,
    y: (top - rect.top) / rect.height,
    width: (right - left) / rect.width,
    height: (bottom - top) / rect.height
  };
  if (Object.values(normalized).some((value) => !finite(value))) return null;
  return {
    x: Math.max(0, Math.min(1, normalized.x)),
    y: Math.max(0, Math.min(1, normalized.y)),
    width: Math.max(0, Math.min(1 - normalized.x, normalized.width)),
    height: Math.max(0, Math.min(1 - normalized.y, normalized.height))
  };
}

export function estimateSourceCropSize(
  rect: NormalizedCropRect,
  sourceWidth: number | null | undefined,
  sourceHeight: number | null | undefined
): EstimatedCropSize | null {
  if (!sourceWidth || !sourceHeight || !Number.isInteger(sourceWidth) || !Number.isInteger(sourceHeight)) {
    return null;
  }
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  const left = Math.floor(rect.x * sourceWidth);
  const top = Math.floor(rect.y * sourceHeight);
  const right = Math.ceil((rect.x + rect.width) * sourceWidth);
  const bottom = Math.ceil((rect.y + rect.height) * sourceHeight);
  return {
    width: Math.max(0, right - left),
    height: Math.max(0, bottom - top)
  };
}

export function isEstimatedCropLargeEnough(size: EstimatedCropSize | null, minimum = 32) {
  return size === null || (size.width >= minimum && size.height >= minimum);
}

export function clientRectSizesMateriallyDiffer(a: ClientRectLike, b: ClientRectLike) {
  if (!isUsableClientRect(a) || !isUsableClientRect(b)) return true;
  return Math.abs(a.width - b.width) > SIZE_EPSILON
    || Math.abs(a.height - b.height) > SIZE_EPSILON;
}
