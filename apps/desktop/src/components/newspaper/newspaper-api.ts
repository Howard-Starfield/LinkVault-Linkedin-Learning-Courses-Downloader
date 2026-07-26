import { invoke } from "@tauri-apps/api/core";

export type NewspaperLibraryStatus = "all" | "completed" | "partial" | "optimizing";
export type NewspaperEditionKind = "all" | "daily" | "weekly" | "special";

export type NewspaperLibraryItem = {
  jobId: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  status: "completed" | "partial" | "optimizing";
  outputDir: string;
  pageCount: number;
  completedCount: number;
  warning?: string | null;
  updatedAt: number;
  thumbnailReady: boolean;
  thumbnailUrl?: string | null;
  thumbnailVersion?: string | null;
  lastPageId?: string | null;
  lastPageIndex?: number | null;
  furthestPageIndex?: number | null;
  readPageCount: number;
  readingUpdatedAt?: number | null;
};

export type NewspaperLibraryPage = {
  items: NewspaperLibraryItem[];
  total: number;
  offset: number;
  limit: number;
  revision: number;
};

export type NewspaperReaderPage = {
  id: string;
  jobId: string;
  canonicalIndex: number;
  pageNumber: string;
  sectionName?: string | null;
  status: string;
  mediaUrl?: string | null;
  mediaVersion: number;
  pixelWidth?: number | null;
  pixelHeight?: number | null;
  finalBytes?: number | null;
  error?: string | null;
};

export type NewspaperReadingProgress = {
  jobId: string;
  lastPageId: string;
  lastPageIndex: number;
  furthestPageIndex: number;
  readPageCount: number;
  updatedAt: number;
};

export type EnsureThumbnailResult =
  | {
      status: "ready" | "generated";
      thumbnailUrl: string;
      thumbnailVersion: string;
      width: number;
      height: number;
    }
  | {
      status: "busy";
      retryAfterMs: number;
    };

export function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

export function getLibraryPage(request: {
  query: string;
  kind: NewspaperEditionKind;
  status: NewspaperLibraryStatus;
  offset: number;
  limit: number;
}) {
  return invoke<NewspaperLibraryPage>("get_newspaper_library_page", request);
}

export function ensureThumbnail(jobId: string) {
  return invoke<EnsureThumbnailResult>("ensure_newspaper_thumbnail", { jobId });
}

export function getReaderManifest(jobId: string) {
  return invoke<NewspaperReaderPage[]>("get_newspaper_reader_manifest", { jobId });
}

export function saveReadingProgress(jobId: string, pageId: string) {
  return invoke<NewspaperReadingProgress>("save_newspaper_reading_progress", {
    jobId,
    pageId
  });
}
