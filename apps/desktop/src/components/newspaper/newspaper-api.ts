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

/** Backend-only Phase 2 contract. Reader selection UI remains deliberately
 * absent until its separately approved phase. */
export type NormalizedCropRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type CreateNewspaperClippingRequest = {
  operationId: string;
  pageId: string;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
};

export type CreateNewspaperClippingResponse = {
  clippingId: string;
  title: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  pageNumber: string;
  imageUrl: string;
  assetVersion: number;
  assetWidth: number;
  assetHeight: number;
  assetByteCount: number;
  revision: number;
  createdAt: number;
};

export type CreateNewspaperClippingFailure = {
  code: string;
  safeMessage: string;
  retryable: boolean;
  operationId: string;
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

export type ClippingAssetState = "creating" | "ready" | "missing" | "delete_pending";
export type ClippingRootStatus = "unchecked" | "connected" | "offline" | "marker_mismatch";
export type NewspaperClippingMatchField = "title" | "note" | "edition" | "date" | "page";

export type NewspaperClippingSummary = {
  id: string;
  title: string;
  noteExcerpt: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  pageNumber: string;
  thumbnailReady: boolean;
  thumbnailUrl?: string | null;
  thumbnailVersion?: string | null;
  sourceAvailable: boolean;
  assetState: ClippingAssetState;
  assetErrorCode?: string | null;
  assetVersion: number;
  assetWidth: number;
  assetHeight: number;
  revision: number;
  createdAt: number;
  updatedAt: number;
};

export type NewspaperClippingsPage = {
  items: NewspaperClippingSummary[];
  total: number;
  offset: number;
  limit: number;
};

export type NewspaperClippingDetail = {
  id: string;
  title: string;
  noteMarkdown: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  pageNumber: string;
  imageUrl: string;
  sourceAvailable: boolean;
  assetState: ClippingAssetState;
  assetErrorCode?: string | null;
  storageStatus: ClippingRootStatus;
  assetWidth: number;
  assetHeight: number;
  revision: number;
  createdAt: number;
  updatedAt: number;
};

export type NewspaperClippingSearchSnippet = {
  field: NewspaperClippingMatchField;
  parts: Array<{ text: string; highlighted: boolean }>;
};

export type NewspaperClippingSearchResult = {
  clipping: NewspaperClippingSummary;
  matchedFields: NewspaperClippingMatchField[];
  snippets: NewspaperClippingSearchSnippet[];
  possibleMatch: boolean;
};

export type NewspaperClippingSearchPage = {
  items: NewspaperClippingSearchResult[];
  total: number;
  offset: number;
  limit: number;
  noteSearchApplied: boolean;
  revision: number;
};

export type NewspaperClippingPossibleMatches = {
  items: NewspaperClippingSearchResult[];
  limit: number;
  revision: number;
};

export type NewspaperSnapshotRoot = {
  rootId: string;
  kind: string;
  displayPath: string;
  status: ClippingRootStatus;
  lastCheckedAt?: number | null;
};

export type ReconnectNewspaperSnapshotRootResult =
  | { status: "cancelled" }
  | { status: "connected"; root: NewspaperSnapshotRoot };

export type EnsureNewspaperClippingThumbnailResponse = {
  status: "ready" | "generated";
  thumbnailUrl: string;
  thumbnailVersion: string;
  width: number;
  height: number;
};

type NewspaperClippingsBrowserHarness = {
  getPage?: typeof getNewspaperClippingsPage;
  getDetail?: typeof getNewspaperClipping;
  update?: typeof updateNewspaperClipping;
  ensureThumbnail?: typeof ensureNewspaperClippingThumbnail;
  search?: typeof searchNewspaperClippings;
  searchPossible?: typeof searchPossibleNewspaperClippings;
  listRoots?: typeof listNewspaperSnapshotRoots;
  checkRoot?: typeof checkNewspaperSnapshotRoot;
  reconnectRoot?: typeof reconnectNewspaperSnapshotRoot;
  openRoot?: typeof openNewspaperSnapshotRoot;
};

function clippingBrowserHarness() {
  if (typeof window === "undefined" || window.location.hostname !== "127.0.0.1") return undefined;
  return (window as Window & { __NEWSPAPER_CLIPPINGS_API__?: NewspaperClippingsBrowserHarness })
    .__NEWSPAPER_CLIPPINGS_API__;
}

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

export function createNewspaperClipping(request: CreateNewspaperClippingRequest) {
  return invoke<CreateNewspaperClippingResponse>("create_newspaper_clipping", {
    request
  });
}

export function getNewspaperClippingsPage(request: {
  query: string;
  sort: "updated_desc" | "created_desc" | "publication_desc" | "title_asc";
  offset: number;
  limit: number;
}) {
  const harness = clippingBrowserHarness()?.getPage;
  if (harness) return harness(request);
  return invoke<NewspaperClippingsPage>("get_newspaper_clippings_page", { request });
}

export function getNewspaperClipping(clippingId: string) {
  const harness = clippingBrowserHarness()?.getDetail;
  if (harness) return harness(clippingId);
  return invoke<NewspaperClippingDetail>("get_newspaper_clipping", { clippingId });
}

export function updateNewspaperClipping(request: {
  clippingId: string;
  expectedRevision: number;
  title: string;
  noteMarkdown: string;
  checkpoint?: { writerSessionId: string; writerSequence: number };
}) {
  const harness = clippingBrowserHarness()?.update;
  if (harness) return harness(request);
  return invoke<NewspaperClippingDetail>("update_newspaper_clipping", { request });
}

export function ensureNewspaperClippingThumbnail(clippingId: string) {
  const harness = clippingBrowserHarness()?.ensureThumbnail;
  if (harness) return harness(clippingId);
  return invoke<EnsureNewspaperClippingThumbnailResponse>(
    "ensure_newspaper_clipping_thumbnail",
    { clippingId }
  );
}

export function searchNewspaperClippings(query: string, offset: number) {
  const harness = clippingBrowserHarness()?.search;
  if (harness) return harness(query, offset);
  return invoke<NewspaperClippingSearchPage>("search_newspaper_clippings", {
    request: { query, offset, limit: 50 }
  });
}

export function searchPossibleNewspaperClippings(query: string) {
  const harness = clippingBrowserHarness()?.searchPossible;
  if (harness) return harness(query);
  return invoke<NewspaperClippingPossibleMatches>("search_possible_newspaper_clippings", {
    request: { query }
  });
}

export function listNewspaperSnapshotRoots() {
  const harness = clippingBrowserHarness()?.listRoots;
  if (harness) return harness();
  return invoke<NewspaperSnapshotRoot[]>("list_newspaper_snapshot_roots");
}

export function checkNewspaperSnapshotRoot(rootId: string) {
  const harness = clippingBrowserHarness()?.checkRoot;
  if (harness) return harness(rootId);
  return invoke<NewspaperSnapshotRoot>("check_newspaper_snapshot_root", { rootId });
}

export function reconnectNewspaperSnapshotRoot(rootId: string) {
  const harness = clippingBrowserHarness()?.reconnectRoot;
  if (harness) return harness(rootId);
  return invoke<ReconnectNewspaperSnapshotRootResult>("reconnect_newspaper_snapshot_root", {
    rootId
  });
}

export function openNewspaperSnapshotRoot(rootId: string) {
  const harness = clippingBrowserHarness()?.openRoot;
  if (harness) return harness(rootId);
  return invoke<void>("open_newspaper_snapshot_root", { rootId });
}

export function clippingErrorCode(error: unknown) {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "code" in error) {
    return String((error as { code: unknown }).code);
  }
  return "CLIPPING_SERVICE_UNAVAILABLE";
}
