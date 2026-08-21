import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  CancelYouTubeRunRequest,
  GetYouTubeDownloadStateRequest,
  GetYouTubeHelperStatusResponse,
  ScanYouTubeSourceRequest,
  ScanYouTubeSourceResponse,
  StartYouTubeDownloadRequest,
  StartYouTubeDownloadResponse,
  YouTubeItemState,
  YouTubeProgressEvent,
  YouTubeProgressItem,
  YouTubeRunSnapshot,
  YouTubeScanItem
} from "./types";

const PREVIEW_SCAN_KEY = "linkvault.youtube.preview.scan";
const PREVIEW_RUN_KEY = "linkvault.youtube.preview.run";
const PREVIEW_EVENT_NAME = "linkvault://youtube-run-changed";
const previewTimers = new Set<number>();

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function getYouTubeHelperStatus(): Promise<GetYouTubeHelperStatusResponse> {
  if (isTauriRuntime()) {
    return invoke<GetYouTubeHelperStatusResponse>("get_youtube_helper_status");
  }
  return { status: "ready", code: null, message: "" };
}

export async function scanYouTubeSource(
  request: ScanYouTubeSourceRequest
): Promise<ScanYouTubeSourceResponse> {
  if (isTauriRuntime()) {
    return invoke<ScanYouTubeSourceResponse>("scan_youtube_source", { request });
  }
  return scanPreviewSource(request);
}

export async function startYouTubeDownload(
  request: StartYouTubeDownloadRequest
): Promise<StartYouTubeDownloadResponse> {
  if (isTauriRuntime()) {
    return invoke<StartYouTubeDownloadResponse>("start_youtube_download", { request });
  }
  return startPreviewDownload(request);
}

export async function getYouTubeDownloadState(
  request: GetYouTubeDownloadStateRequest
): Promise<YouTubeRunSnapshot | null> {
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot | null>("get_youtube_download_state", { request });
  }
  return readPreviewRun();
}

export async function cancelYouTubeDownload(
  request: CancelYouTubeRunRequest
): Promise<YouTubeRunSnapshot | null> {
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot | null>("cancel_youtube_download", { request });
  }
  return cancelPreviewDownload(request.runId);
}

export async function subscribeYouTubeRunChanged(
  handler: (event: YouTubeProgressEvent) => void
): Promise<() => void> {
  if (isTauriRuntime()) {
    return listen<YouTubeProgressEvent>(PREVIEW_EVENT_NAME, (event) => handler(event.payload));
  }
  const onPreviewEvent = (event: Event) => {
    if (!(event instanceof CustomEvent)) return;
    const payload: unknown = event.detail;
    if (isYouTubeProgressEvent(payload)) handler(payload);
  };
  window.addEventListener(PREVIEW_EVENT_NAME, onPreviewEvent);
  return () => window.removeEventListener(PREVIEW_EVENT_NAME, onPreviewEvent);
}

function readStoredValue<T>(key: string, guard: (value: unknown) => value is T): T | null {
  if (typeof window === "undefined") return null;
  const stored = window.sessionStorage.getItem(key);
  if (!stored) return null;
  try {
    const parsed: unknown = JSON.parse(stored);
    return guard(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function writeStoredValue(key: string, value: unknown): void {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(key, JSON.stringify(value));
}

function readPreviewScan(): ScanYouTubeSourceResponse | null {
  return readStoredValue(PREVIEW_SCAN_KEY, isScanResponse);
}

function readPreviewRun(): YouTubeRunSnapshot | null {
  return readStoredValue(PREVIEW_RUN_KEY, isRunSnapshot);
}

function dispatchPreviewEvent(snapshot: YouTubeRunSnapshot): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<YouTubeProgressEvent>(PREVIEW_EVENT_NAME, { detail: snapshot }));
}

function scanPreviewSource(request: ScanYouTubeSourceRequest): ScanYouTubeSourceResponse {
  const parsed = parseYouTubeUrl(request.url);
  const playlist = request.playlistMode === "playlist" || parsed.playlistId !== null;
  const itemCount = playlist ? 4 : 1;
  const items: YouTubeScanItem[] = Array.from({ length: itemCount }, (_, index) => {
    const ordinal = index + 1;
    const videoId = playlist ? `${parsed.videoId}-${ordinal}` : parsed.videoId;
    return {
      occurrenceId: `preview-occurrence-${parsed.sourceId}-${ordinal}`,
      videoId,
      sourceUrl: `https://www.youtube.com/watch?v=${encodeURIComponent(videoId)}`,
      title: playlist ? `Preview source ${ordinal}` : "Preview YouTube source",
      ordinal,
      channelName: "LinkVault Preview",
      channelId: null,
      durationSeconds: 90 + ordinal * 30,
      thumbnailAvailable: false,
      availability: "available",
      metadataDigest: `preview-metadata-${parsed.sourceId}-${ordinal}`
    };
  });
  const response: ScanYouTubeSourceResponse = {
    scanPlanId: `preview-scan-${Date.now()}`,
    expiresAt: new Date(Date.now() + 15 * 60_000).toISOString(),
    kind: playlist ? "playlist" : "video",
    title: playlist ? "Preview playlist" : "Preview YouTube source",
    sourceId: parsed.sourceId,
    canonicalUrl: parsed.canonicalUrl,
    playlistId: parsed.playlistId,
    truncated: false,
    items
  };
  writeStoredValue(PREVIEW_SCAN_KEY, response);
  return response;
}

function startPreviewDownload(request: StartYouTubeDownloadRequest): StartYouTubeDownloadResponse {
  const scan = readPreviewScan();
  if (!scan || scan.scanPlanId !== request.scanPlanId) {
    throw new Error("The preview scan has expired. Scan the source again.");
  }
  const selected = new Set(request.selectedOccurrenceIds);
  const selectedItems = scan.items.filter((item) => selected.has(item.occurrenceId));
  if (selectedItems.length === 0) throw new Error("Select at least one source occurrence.");
  clearPreviewTimers();
  const runId = `preview-run-${Date.now()}`;
  const snapshot: YouTubeRunSnapshot = {
    clientSubmissionId: request.clientSubmissionId,
    planFingerprint: `preview-plan-${scan.scanPlanId}`,
    schemaVersion: 1,
    runId,
    revision: 1,
    state: "running",
    item: selectedItems[0] ? toProgressItem(selectedItems[0], "running", "media") : null,
    progress: { bytesCompleted: 0, bytesTotal: 1, fraction: 0 },
    counts: { completed: 0, completedWithWarnings: 0, selected: selectedItems.length, failed: 0, skipped: 0, cancelled: 0 },
    warnings: [],
    error: null,
    items: selectedItems.map((item) => ({
      occurrenceId: item.occurrenceId,
      artifactFingerprint: `preview-artifact-${item.occurrenceId}`,
      videoId: item.videoId,
      ordinal: item.ordinal,
      title: item.title,
      state: "pending",
      phase: "waiting",
      warnings: [],
      error: null,
      publishedArtifactKinds: []
    }))
  };
  writeStoredValue(PREVIEW_RUN_KEY, snapshot);
  dispatchPreviewEvent(snapshot);
  selectedItems.forEach((item, index) => {
    const timer = window.setTimeout(() => {
      previewTimers.delete(timer);
      const current = readPreviewRun();
      if (!current || current.runId !== runId || current.state === "cancelled") return;
      const nextItems = current.items.map((candidate) => candidate.occurrenceId === item.occurrenceId
        ? { ...candidate, state: "completed" as const, phase: "completed" as const, publishedArtifactKinds: ["media", "metadata"] as Array<"media" | "vtt" | "transcript_json" | "metadata"> }
        : candidate);
      const completed = nextItems.filter((candidate) => candidate.state === "completed").length;
      const terminal = completed === nextItems.length;
      const next: YouTubeRunSnapshot = {
        ...current,
        revision: current.revision + 1,
        state: terminal ? "completed" : "running",
        item: terminal ? null : toProgressItem(selectedItems[index + 1] ?? item, "running", "media"),
        progress: { bytesCompleted: completed, bytesTotal: nextItems.length, fraction: completed / nextItems.length },
        counts: { ...current.counts, completed },
        items: nextItems
      };
      writeStoredValue(PREVIEW_RUN_KEY, next);
      dispatchPreviewEvent(next);
    }, 360 * (index + 1));
    previewTimers.add(timer);
  });
  return {
    clientSubmissionId: request.clientSubmissionId,
    runId,
    revision: snapshot.revision,
    scanPlanId: request.scanPlanId,
    planFingerprint: snapshot.planFingerprint,
    state: "running"
  };
}

function cancelPreviewDownload(runId: string): YouTubeRunSnapshot | null {
  const current = readPreviewRun();
  if (!current || current.runId !== runId) return current;
  clearPreviewTimers();
  const items = current.items.map((item) => item.state === "completed" || item.state === "completed_with_warnings"
    ? item
    : { ...item, state: "cancelled" as const, phase: "cancelled" as const });
  const cancelled = items.filter((item) => item.state === "cancelled").length;
  const next: YouTubeRunSnapshot = {
    ...current,
    revision: current.revision + 1,
    state: "cancelled",
    item: null,
    progress: { bytesCompleted: null, bytesTotal: null, fraction: null },
    counts: { ...current.counts, cancelled },
    items
  };
  writeStoredValue(PREVIEW_RUN_KEY, next);
  dispatchPreviewEvent(next);
  return next;
}

function clearPreviewTimers(): void {
  previewTimers.forEach((timer) => window.clearTimeout(timer));
  previewTimers.clear();
}

function toProgressItem(item: YouTubeScanItem, state: YouTubeItemState, phase: YouTubeProgressItem["phase"]): YouTubeProgressEvent["item"] {
  return {
    occurrenceId: item.occurrenceId,
    artifactFingerprint: `preview-artifact-${item.occurrenceId}`,
    videoId: item.videoId,
    ordinal: item.ordinal,
    title: item.title,
    state,
    phase
  };
}

function parseYouTubeUrl(raw: string): { canonicalUrl: string; sourceId: string; videoId: string; playlistId: string | null } {
  const trimmed = raw.trim();
  const withProtocol = /^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  let url: URL;
  try {
    url = new URL(withProtocol);
  } catch {
    throw new Error("Enter a valid YouTube URL.");
  }
  const hostname = url.hostname.toLowerCase().replace(/^www\./, "");
  if (hostname !== "youtube.com" && hostname !== "youtu.be" && hostname !== "m.youtube.com") {
    throw new Error("Use a youtube.com or youtu.be URL.");
  }
  const pathParts = url.pathname.split("/").filter(Boolean);
  const lastPathPart = pathParts.length > 0 ? pathParts[pathParts.length - 1] : "";
  const videoId = hostname === "youtu.be"
    ? url.pathname.slice(1).split("/")[0]
    : url.searchParams.get("v") ?? lastPathPart;
  const playlistId = url.searchParams.get("list");
  if (!videoId && !playlistId) throw new Error("The YouTube URL does not contain a video or playlist.");
  const sourceId = playlistId ?? videoId;
  return {
    canonicalUrl: `https://www.youtube.com/${playlistId ? "playlist?list=" : "watch?v="}${encodeURIComponent(sourceId)}`,
    sourceId,
    videoId: videoId || `preview-${sourceId}`,
    playlistId
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isScanResponse(value: unknown): value is ScanYouTubeSourceResponse {
  if (!isRecord(value) || typeof value.scanPlanId !== "string" || !Array.isArray(value.items)) return false;
  return value.items.every((item) => isRecord(item) && typeof item.occurrenceId === "string" && typeof item.title === "string");
}

function isYouTubeProgressEvent(value: unknown): value is YouTubeProgressEvent {
  return isRecord(value)
    && value.schemaVersion === 1
    && typeof value.runId === "string"
    && typeof value.revision === "number"
    && typeof value.state === "string"
    && isRecord(value.counts);
}

function isRunSnapshot(value: unknown): value is YouTubeRunSnapshot {
  if (!isRecord(value) || !isYouTubeProgressEvent(value)) return false;
  return typeof value.clientSubmissionId === "string"
    && typeof value.planFingerprint === "string"
    && Array.isArray(value.items);
}
