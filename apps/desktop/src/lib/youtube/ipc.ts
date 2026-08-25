import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  CancelYouTubeRunRequest,
  GetYouTubeDownloadStateRequest,
  GetYouTubeHelperStatusResponse,
  InspectYouTubeTranscriptsRequest,
  InspectYouTubeTranscriptsResponse,
  MutateYouTubeRunRequest,
  SavedYouTubePreferences,
  ScanYouTubeSourceRequest,
  ScanYouTubeSourceResponse,
  StartYouTubeDownloadRequest,
  StartYouTubeDownloadResponse,
  YouTubeDownloadMode,
  YouTubeItemState,
  YouTubeProgressEvent,
  YouTubeProgressItem,
  YouTubeRunSnapshot,
  YouTubeScanItem,
  YouTubeError,
  YouTubeTranscriptOccurrence,
  YouTubeTranscriptTrack
} from "./types";

const PREVIEW_SCAN_KEY = "linkvault.youtube.preview.scan";
const PREVIEW_SCANS_KEY = "linkvault.youtube.preview.scans";
const PREVIEW_RUN_KEY = "linkvault.youtube.preview.run";
const PREVIEW_EVENT_NAME = "linkvault://youtube-run-changed";
export const YOUTUBE_UI_MOCK_RUN_PREFIX = "youtube-ui-mock-";
let previewRunIdPrefix = "preview-run-";
const previewTimers = new Set<number>();
const previewRunModes = new Map<string, YouTubeDownloadMode>();

function isYouTubeUiMockRun(runId: string): boolean {
  return runId.startsWith(YOUTUBE_UI_MOCK_RUN_PREFIX);
}

function readUiMockRun(): YouTubeRunSnapshot | null {
  const snapshot = readPreviewRun();
  return snapshot && isYouTubeUiMockRun(snapshot.runId) ? snapshot : null;
}

export function readActiveYouTubePreviewScan(): ScanYouTubeSourceResponse | null {
  return readPreviewScan();
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function formatYouTubeInvokeError(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }
  if (typeof error === "string" && error.trim()) {
    return error;
  }
  if (isRecord(error)) {
    const nested = error.error;
    if (isRecord(nested)) {
      const nestedMessage = formatYouTubeInvokeError(nested);
      if (!nestedMessage.startsWith("An unexpected")) return nestedMessage;
    }
    const code = typeof error.code === "string" ? error.code.trim() : "";
    const message = typeof error.message === "string" ? error.message.trim() : "";
    if (code && message) return `${code}: ${message}`;
    if (message) return message;
    if (code) return code;
  }
  return "An unexpected YouTube error occurred.";
}

export function youtubeErrorFromUnknown(error: unknown): YouTubeError {
  if (isRecord(error) && typeof error.code === "string" && typeof error.message === "string") {
    return { code: error.code, message: error.message };
  }
  return {
    code: "HELPER_STATUS_UNAVAILABLE",
    message: formatYouTubeInvokeError(error)
  };
}

export async function getYouTubeHelperStatus(): Promise<GetYouTubeHelperStatusResponse> {
  if (isTauriRuntime()) {
    return invoke<GetYouTubeHelperStatusResponse>("get_youtube_helper_status");
  }
  return { status: "ready", code: null, message: "" };
}

export async function getYouTubePreferences(): Promise<SavedYouTubePreferences> {
  if (isTauriRuntime()) {
    return invoke<SavedYouTubePreferences>("get_youtube_preferences");
  }
  return { output_dir: "" };
}

export async function saveYouTubePreferences(
  preferences: SavedYouTubePreferences
): Promise<SavedYouTubePreferences> {
  if (isTauriRuntime()) {
    return invoke<SavedYouTubePreferences>("save_youtube_preferences", { preferences });
  }
  return { output_dir: preferences.output_dir.trim() };
}

export async function scanYouTubeSource(
  request: ScanYouTubeSourceRequest
): Promise<ScanYouTubeSourceResponse> {
  if (isTauriRuntime()) {
    return invoke<ScanYouTubeSourceResponse>("scan_youtube_source", { request });
  }
  return scanPreviewSource(request);
}

export async function inspectYouTubeTranscripts(
  request: InspectYouTubeTranscriptsRequest
): Promise<InspectYouTubeTranscriptsResponse> {
  if (isTauriRuntime()) {
    return invoke<InspectYouTubeTranscriptsResponse>("inspect_youtube_transcripts", { request });
  }
  return inspectPreviewTranscripts(request);
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
  const mock = readUiMockRun();
  if (mock && (request.runId === null || request.runId === mock.runId)) {
    return mock;
  }
  if (request.runId && isYouTubeUiMockRun(request.runId)) {
    return mock && mock.runId === request.runId ? mock : null;
  }
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot | null>("get_youtube_download_state", { request });
  }
  return readPreviewRun();
}

export async function cancelYouTubeDownload(
  request: CancelYouTubeRunRequest
): Promise<YouTubeRunSnapshot | null> {
  if (isYouTubeUiMockRun(request.runId)) {
    return cancelPreviewDownload(request.runId);
  }
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot | null>("cancel_youtube_download", { request });
  }
  return cancelPreviewDownload(request.runId);
}

export async function pauseYouTubeDownload(
  request: MutateYouTubeRunRequest
): Promise<YouTubeRunSnapshot> {
  if (isYouTubeUiMockRun(request.runId)) {
    return pausePreviewDownload(request);
  }
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot>("pause_youtube_download", { request });
  }
  return pausePreviewDownload(request);
}

export async function resumeYouTubeDownload(
  request: MutateYouTubeRunRequest
): Promise<YouTubeRunSnapshot> {
  if (isYouTubeUiMockRun(request.runId)) {
    return resumePreviewDownload(request);
  }
  if (isTauriRuntime()) {
    return invoke<YouTubeRunSnapshot>("resume_youtube_download", { request });
  }
  return resumePreviewDownload(request);
}

export async function startYouTubeUiMock(): Promise<StartYouTubeDownloadResponse> {
  previewRunIdPrefix = YOUTUBE_UI_MOCK_RUN_PREFIX;
  try {
    const scan = buildUiMockScan();
    writePreviewScan(scan);
    return startPreviewDownload({
      clientSubmissionId: `youtube-ui-mock-${Date.now()}`,
      scanPlanId: scan.scanPlanId,
      selectedOccurrenceIds: scan.items.map((item) => item.occurrenceId),
      outputDir: "C:\\Users\\Public\\Videos",
      mode: "video_and_transcript",
      maxHeight: 1080,
      preferredLanguage: "en",
      fallbackLanguages: ["en"],
      allowAutomaticCaptions: true,
      continueWithoutTranscript: true
    });
  } finally {
    previewRunIdPrefix = "preview-run-";
  }
}

export async function subscribeYouTubeRunChanged(
  handler: (event: YouTubeProgressEvent) => void
): Promise<() => void> {
  const onPreviewEvent = (event: Event) => {
    if (!(event instanceof CustomEvent)) return;
    const payload: unknown = event.detail;
    if (isYouTubeProgressEvent(payload)) handler(payload);
  };
  window.addEventListener(PREVIEW_EVENT_NAME, onPreviewEvent);
  let unlistenTauri: (() => void) | undefined;
  if (isTauriRuntime()) {
    unlistenTauri = await listen<YouTubeProgressEvent>(PREVIEW_EVENT_NAME, (event) => handler(event.payload));
  }
  return () => {
    window.removeEventListener(PREVIEW_EVENT_NAME, onPreviewEvent);
    unlistenTauri?.();
  };
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

function readPreviewScans(): Record<string, ScanYouTubeSourceResponse> {
  return readStoredValue(PREVIEW_SCANS_KEY, isScanMap) ?? {};
}

function writePreviewScan(response: ScanYouTubeSourceResponse): void {
  const all = readPreviewScans();
  all[response.scanPlanId] = response;
  writeStoredValue(PREVIEW_SCANS_KEY, all);
  writeStoredValue(PREVIEW_SCAN_KEY, response);
}

function findPreviewScan(scanPlanId: string): ScanYouTubeSourceResponse | null {
  return readPreviewScans()[scanPlanId] ?? readPreviewScan();
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
  const playlist = request.playlistMode === "playlist"
    || (request.playlistMode === undefined && parsed.videoId === null && parsed.playlistId !== null);
  const sourceId = playlist ? parsed.playlistId : parsed.videoId;
  if (!sourceId) {
    throw new Error(playlist
      ? "The preview URL does not contain a playlist id."
      : "The preview URL does not contain a video id.");
  }
  const itemCount = playlist ? 4 : 1;
  const items: YouTubeScanItem[] = Array.from({ length: itemCount }, (_, index) => {
    const ordinal = index + 1;
    const videoId = playlist ? `${parsed.videoId ?? `preview-${sourceId}`}-${ordinal}` : sourceId;
    return {
      occurrenceId: `preview-occurrence-${sourceId}-${ordinal}`,
      videoId,
      sourceUrl: `https://www.youtube.com/watch?v=${encodeURIComponent(videoId)}`,
      title: playlist ? `Video ${ordinal}` : sourceId,
      ordinal,
      channelName: null,
      channelId: null,
      durationSeconds: 90 + ordinal * 30,
      thumbnailAvailable: false,
      availability: "available",
      metadataDigest: `preview-metadata-${sourceId}-${ordinal}`
    };
  });
  const response: ScanYouTubeSourceResponse = {
    scanPlanId: `preview-scan-${Date.now()}`,
    expiresAt: new Date(Date.now() + 15 * 60_000).toISOString(),
    kind: playlist ? "playlist" : "video",
    title: playlist ? "Playlist" : sourceId,
    sourceId,
    canonicalUrl: playlist
      ? `https://www.youtube.com/playlist?list=${encodeURIComponent(sourceId)}`
      : `https://www.youtube.com/watch?v=${encodeURIComponent(sourceId)}`,
    playlistId: playlist ? parsed.playlistId : null,
    truncated: false,
    items
  };
  writePreviewScan(response);
  return response;
}

function inspectPreviewTranscripts(
  request: InspectYouTubeTranscriptsRequest
): InspectYouTubeTranscriptsResponse {
  const scan = findPreviewScan(request.scanPlanId);
  if (!scan || scan.scanPlanId !== request.scanPlanId) {
    throw new Error("The preview scan has expired. Scan the source again.");
  }
  const selected = new Set<string>();
  const occurrences: YouTubeTranscriptOccurrence[] = [];
  for (const occurrenceId of request.occurrenceIds) {
    if (selected.has(occurrenceId)) {
      throw new Error("An occurrence was selected more than once.");
    }
    selected.add(occurrenceId);
    const item = scan.items.find((candidate) => candidate.occurrenceId === occurrenceId);
    if (!item) throw new Error("The selected occurrence is no longer in the scan plan.");
    const tracks: YouTubeTranscriptTrack[] = [
      {
        trackKey: `${occurrenceId}:en:uploader`,
        languageTag: "en",
        displayLanguage: "English",
        source: "uploader",
        isLikelyTranslated: false,
        formats: ["vtt"]
      },
      {
        trackKey: `${occurrenceId}:en:automatic`,
        languageTag: "en",
        displayLanguage: "English (automatic)",
        source: "automatic",
        isLikelyTranslated: false,
        formats: ["vtt"]
      }
    ];
    occurrences.push({ occurrenceId: item.occurrenceId, videoId: item.videoId, tracks });
  }
  return { occurrences };
}

function buildUiMockScan(): ScanYouTubeSourceResponse {
  const sourceId = "MOCKPLAYLIST";
  const titles = [
    "01 - Attention is all you need",
    "02 - Transformers in practice",
    "03 - Fine-tuning without the drama",
    "04 - Evaluating retrieval quality"
  ];
  const items: YouTubeScanItem[] = titles.map((title, index) => {
    const ordinal = index + 1;
    const videoId = `mock-video-${ordinal}`;
    return {
      occurrenceId: `youtube-ui-mock-${sourceId}-${ordinal}`,
      videoId,
      sourceUrl: `https://www.youtube.com/watch?v=${videoId}`,
      title,
      ordinal,
      channelName: "LinkVault mock channel",
      channelId: "mock-channel",
      durationSeconds: 240 + ordinal * 45,
      thumbnailAvailable: false,
      availability: "available",
      metadataDigest: `youtube-ui-mock-metadata-${ordinal}`
    };
  });
  return {
    scanPlanId: `youtube-ui-mock-scan-${Date.now()}`,
    expiresAt: new Date(Date.now() + 15 * 60_000).toISOString(),
    kind: "playlist",
    title: "Mock YouTube playlist",
    sourceId,
    canonicalUrl: `https://www.youtube.com/playlist?list=${sourceId}`,
    playlistId: sourceId,
    truncated: false,
    items
  };
}

function startPreviewDownload(request: StartYouTubeDownloadRequest): StartYouTubeDownloadResponse {
  const scan = findPreviewScan(request.scanPlanId);
  if (!scan || scan.scanPlanId !== request.scanPlanId) {
    throw new Error("The preview scan has expired. Scan the source again.");
  }
  const selected = new Set(request.selectedOccurrenceIds);
  const selectedItems = scan.items.filter((item) => selected.has(item.occurrenceId));
  if (selectedItems.length === 0) throw new Error("Select at least one source occurrence.");
  clearPreviewTimers();
  const runId = `${previewRunIdPrefix}${Date.now()}`;
  previewRunModes.set(runId, request.mode);
  const runningPhase = previewPhaseForMode(request.mode);
  const snapshot: YouTubeRunSnapshot = {
    clientSubmissionId: request.clientSubmissionId,
    planFingerprint: `preview-plan-${scan.scanPlanId}`,
    schemaVersion: 1,
    runId,
    revision: 1,
    state: "running",
    item: selectedItems[0] ? toProgressItem(selectedItems[0], "running", runningPhase) : null,
    progress: { bytesCompleted: 0, bytesTotal: selectedItems.length, fraction: 0 },
    counts: { completed: 0, completedWithWarnings: 0, selected: selectedItems.length, failed: 0, skipped: 0, cancelled: 0 },
    warnings: [],
    error: null,
    items: selectedItems.map((item, index) => ({
      occurrenceId: item.occurrenceId,
      artifactFingerprint: `preview-artifact-${item.occurrenceId}`,
      videoId: item.videoId,
      ordinal: item.ordinal,
      title: item.title,
      state: index === 0 ? "running" : "pending",
      phase: index === 0 ? runningPhase : "waiting",
      warnings: [],
      error: null,
      publishedArtifactKinds: []
    }))
  };
  writeStoredValue(PREVIEW_RUN_KEY, snapshot);
  dispatchPreviewEvent(snapshot);
  schedulePreviewAdvance(runId, isYouTubeUiMockRun(runId) ? 780 : 360);
  return {
    receipt: {
      clientSubmissionId: request.clientSubmissionId,
      runId,
      revision: snapshot.revision,
      scanPlanId: request.scanPlanId,
      planFingerprint: snapshot.planFingerprint,
      state: "running"
    },
    replayed: false
  };
}

function schedulePreviewAdvance(runId: string, delay = 360): void {
  const timer = window.setTimeout(() => {
    previewTimers.delete(timer);
    const current = readPreviewRun();
    if (!current || current.runId !== runId || current.state === "cancelled") return;
    if (current.state === "paused") return;

    const pauseAfterCurrent = current.state === "pause_requested";
    const nextIndex = current.items.findIndex((item) => item.state === "pending" || item.state === "running");
    if (nextIndex < 0) return;
    const mode = previewRunModes.get(runId) ?? "video_and_transcript";
    const nextItems = current.items.map((item, index) => {
      if (index === nextIndex) {
        return {
          ...item,
          state: "completed" as const,
          phase: "completed" as const,
          publishedArtifactKinds: previewArtifactKinds(mode)
        };
      }
      return item;
    });
    const completed = nextItems.filter((item) => item.state === "completed").length;
    const terminal = completed === nextItems.length;
    const followOnIndex = !terminal && !pauseAfterCurrent
      ? nextItems.findIndex((item) => item.state === "pending")
      : -1;
    const advancedItems = followOnIndex < 0
      ? nextItems
      : nextItems.map((item, index) => index === followOnIndex
        ? { ...item, state: "running" as const, phase: previewPhaseForMode(mode) }
        : item);
    const nextActive = advancedItems.find((item) => item.state === "running") ?? null;
    const next: YouTubeRunSnapshot = {
      ...current,
      revision: current.revision + 1,
      state: terminal ? "completed" : pauseAfterCurrent ? "paused" : "running",
      item: nextActive
        ? toProgressItemFromOutcome(nextActive, "running", previewPhaseForMode(mode))
        : null,
      progress: { bytesCompleted: completed, bytesTotal: advancedItems.length, fraction: completed / advancedItems.length },
      counts: { ...current.counts, completed },
      items: advancedItems
    };
    writeStoredValue(PREVIEW_RUN_KEY, next);
    dispatchPreviewEvent(next);
    if (terminal) previewRunModes.delete(runId);
    else if (!pauseAfterCurrent) schedulePreviewAdvance(runId, isYouTubeUiMockRun(runId) ? 780 : 360);
  }, delay);
  previewTimers.add(timer);
}

function pausePreviewDownload(request: MutateYouTubeRunRequest): YouTubeRunSnapshot {
  const current = readPreviewMutation(request);
  if (current.state !== "running") throw new Error("The preview run is not running.");
  const next: YouTubeRunSnapshot = {
    ...current,
    revision: current.revision + 1,
    state: "pause_requested"
  };
  writeStoredValue(PREVIEW_RUN_KEY, next);
  dispatchPreviewEvent(next);
  return next;
}

function resumePreviewDownload(request: MutateYouTubeRunRequest): YouTubeRunSnapshot {
  const current = readPreviewMutation(request);
  if (current.state !== "paused" && current.state !== "pause_requested") {
    throw new Error("The preview run is not paused.");
  }
  clearPreviewTimers();
  const next: YouTubeRunSnapshot = {
    ...current,
    revision: current.revision + 1,
    state: "running"
  };
  writeStoredValue(PREVIEW_RUN_KEY, next);
  dispatchPreviewEvent(next);
  schedulePreviewAdvance(next.runId, isYouTubeUiMockRun(next.runId) ? 780 : 360);
  return next;
}

function readPreviewMutation(request: MutateYouTubeRunRequest): YouTubeRunSnapshot {
  const current = readPreviewRun();
  if (!current || current.runId !== request.runId) throw new Error("The preview run is unavailable.");
  if (current.revision !== request.expectedRevision) throw new Error("The preview run changed; refresh its state and retry.");
  return current;
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
  previewRunModes.delete(runId);
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

function toProgressItemFromOutcome(
  item: YouTubeRunSnapshot["items"][number],
  state: YouTubeItemState,
  phase: YouTubeProgressItem["phase"]
): YouTubeProgressEvent["item"] {
  return {
    occurrenceId: item.occurrenceId,
    artifactFingerprint: item.artifactFingerprint,
    videoId: item.videoId,
    ordinal: item.ordinal,
    title: item.title,
    state,
    phase
  };
}

function previewPhaseForMode(mode: YouTubeDownloadMode): YouTubeProgressItem["phase"] {
  return mode === "transcript_only" ? "transcript" : "media";
}

function previewArtifactKinds(mode: YouTubeDownloadMode): YouTubeRunSnapshot["items"][number]["publishedArtifactKinds"] {
  if (mode === "video_only") return ["media", "metadata"];
  if (mode === "transcript_only") return ["vtt", "transcript_json", "metadata"];
  return ["media", "vtt", "transcript_json", "metadata"];
}

function parseYouTubeUrl(raw: string): { videoId: string | null; playlistId: string | null } {
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
    : url.searchParams.get("v") ?? (lastPathPart !== "watch" && lastPathPart !== "playlist" ? lastPathPart : null);
  const playlistId = url.searchParams.get("list");
  if (!videoId && !playlistId) throw new Error("The YouTube URL does not contain a video or playlist.");
  return {
    videoId: videoId || null,
    playlistId
  };
}

function isScanResponse(value: unknown): value is ScanYouTubeSourceResponse {
  if (!isRecord(value) || typeof value.scanPlanId !== "string" || !Array.isArray(value.items)) return false;
  return value.items.every((item) => isRecord(item) && typeof item.occurrenceId === "string" && typeof item.title === "string");
}

function isScanMap(value: unknown): value is Record<string, ScanYouTubeSourceResponse> {
  if (!isRecord(value)) return false;
  return Object.values(value).every((entry) => isScanResponse(entry));
}

function isYouTubeProgressEvent(value: unknown): value is YouTubeProgressEvent {
  return isRecord(value)
    && value.schemaVersion === 1
    && typeof value.runId === "string"
    && typeof value.revision === "number"
    && typeof value.state === "string"
    && isRecord(value.counts);
}

export function isYouTubeRunSnapshot(value: unknown): value is YouTubeRunSnapshot {
  if (!isRecord(value) || !isYouTubeProgressEvent(value)) return false;
  return typeof value.clientSubmissionId === "string"
    && typeof value.planFingerprint === "string"
    && Array.isArray(value.items);
}

function isRunSnapshot(value: unknown): value is YouTubeRunSnapshot {
  return isYouTubeRunSnapshot(value);
}
