import type { NewspaperClippingDetail, NormalizedCropRect } from "./newspaper-api";

export type NewspaperReaderSourceTarget = {
  generation: number;
  jobId: string;
  pageId: string;
  highlight: NormalizedCropRect;
  sourceMediaVersionSnapshot: number;
  returnClippingId: string;
};

export type NewspaperClippingOpenTarget = {
  clippingId: string;
  focusEditor: boolean;
};

export function readerTargetFromClipping(
  detail: NewspaperClippingDetail,
  generation: number
): NewspaperReaderSourceTarget | null {
  if (!detail.sourceAvailable || !detail.sourceJobId || !detail.sourcePageId) return null;
  return {
    generation,
    jobId: detail.sourceJobId,
    pageId: detail.sourcePageId,
    highlight: detail.normalizedRect,
    sourceMediaVersionSnapshot: detail.sourceMediaVersionSnapshot,
    returnClippingId: detail.id
  };
}

export function isCurrentReaderTarget(
  expected: NewspaperReaderSourceTarget | null,
  generation: number
) {
  return expected?.generation === generation;
}
