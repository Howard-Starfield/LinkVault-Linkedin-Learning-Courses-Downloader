export type YouTubePlaylistMode = "video" | "playlist";

export type YouTubeDownloadMode =
  | "video_and_transcript"
  | "video_only"
  | "transcript_only";

export type YouTubeMaxHeight = null | 2160 | 1440 | 1080 | 720 | 480;

/** SQLite `youtube.preferences` payload — snake_case to match the stored JSON. */
export interface SavedYouTubePreferences {
  output_dir: string;
}

export type YouTubeRunState =
  | "running"
  | "pause_requested"
  | "paused"
  | "cancelling"
  | "completed"
  | "completed_with_warnings"
  | "failed"
  | "cancelled";

export type YouTubeItemState =
  | "pending"
  | "running"
  | "completed"
  | "completed_with_warnings"
  | "failed"
  | "cancelled"
  | "skipped"
  | "skipped_existing";

export type YouTubeItemPhase =
  | "waiting"
  | "transcript"
  | "media"
  | "merging"
  | "normalizing_transcript"
  | "verifying"
  | "finalizing"
  | "completed"
  | "warning"
  | "failed"
  | "cancelled";

export type YouTubeWarningCode =
  | "PLAYLIST_TRUNCATED"
  | "TRANSCRIPT_FALLBACK_USED"
  | "TRANSCRIPT_MISSING"
  | "TRANSCRIPT_INVALID"
  | "ITEM_UNAVAILABLE"
  | "METADATA_DRIFT"
  | "PLAYBACK_COMPATIBILITY_WARNING"
  | "PARTIAL_QUARANTINED"
  | "ITEM_FAILED_CONTINUING"
  | "EXISTING_VERIFIED_REUSED";

export interface YouTubeError {
  code: string;
  message: string;
}

export type YouTubeHelperBackendStatus = "ready" | "blocked";

export interface GetYouTubeHelperStatusResponse {
  status: YouTubeHelperBackendStatus;
  code: string | null;
  message: string;
}

export type MediaToolchainComponentKind = "yt_dlp" | "ffmpeg" | "deno";

export interface MediaToolchainComponentStatus {
  kind: MediaToolchainComponentKind;
  available: boolean;
  path: string | null;
}

export interface MediaToolchainStatus {
  ready: boolean;
  media_core_ready: boolean;
  web_media_ready: boolean;
  managed_install_present: boolean;
  active_version: string | null;
  previous_version: string | null;
  last_checked_at: string | null;
  latest_available: string | null;
  components: MediaToolchainComponentStatus[];
  error: string | null;
}

export interface YouTubeScanItem {
  occurrenceId: string;
  videoId: string;
  sourceUrl: string;
  title: string;
  ordinal: number;
  channelName: string | null;
  channelId: string | null;
  durationSeconds: number | null;
  thumbnailAvailable: boolean;
  availability: "available" | "unavailable" | "unknown";
  metadataDigest: string;
}

export interface ScanYouTubeSourceRequest {
  clientOperationId: string;
  url: string;
  playlistMode?: YouTubePlaylistMode;
}

export interface ScanYouTubeSourceResponse {
  scanPlanId: string;
  expiresAt: string;
  kind: YouTubePlaylistMode;
  title: string;
  sourceId: string;
  canonicalUrl: string;
  playlistId: string | null;
  /** More entries exist than the scan keeps (500). */
  truncated: boolean;
  /** The playlist's total size when YouTube reports it. */
  totalCount: number | null;
  items: YouTubeScanItem[];
}

export type YouTubeTranscriptSource = "uploader" | "automatic";

export interface InspectYouTubeTranscriptsRequest {
  clientOperationId: string;
  scanPlanId: string;
  occurrenceIds: string[];
}

export interface YouTubeTranscriptTrack {
  trackKey: string;
  languageTag: string;
  displayLanguage: string;
  source: YouTubeTranscriptSource;
  isLikelyTranslated: boolean;
  formats: string[];
}

export interface YouTubeTranscriptOccurrence {
  occurrenceId: string;
  videoId: string;
  tracks: YouTubeTranscriptTrack[];
}

export interface InspectYouTubeTranscriptsResponse {
  occurrences: YouTubeTranscriptOccurrence[];
}

export interface StartYouTubeDownloadRequest {
  clientSubmissionId: string;
  scanPlanId: string;
  selectedOccurrenceIds: string[];
  outputDir: string;
  mode: YouTubeDownloadMode;
  maxHeight: YouTubeMaxHeight;
  preferredLanguage: string | null;
  fallbackLanguages: string[];
  allowAutomaticCaptions: boolean;
  continueWithoutTranscript: boolean;
}

export interface YouTubeStartReceipt {
  clientSubmissionId: string;
  runId: string;
  revision: number;
  scanPlanId: string;
  planFingerprint: string;
  state: "running";
}

export interface StartYouTubeDownloadResponse {
  receipt: YouTubeStartReceipt;
  replayed: boolean;
}

export interface GetYouTubeDownloadStateRequest {
  runId: string | null;
}

export interface CancelYouTubeRunRequest {
  runId: string;
}

export interface MutateYouTubeRunRequest {
  runId: string;
  expectedRevision: number;
}

/**
 * Open the download location for a completed occurrence.
 * V1 snapshots do not expose per-item media paths, so the backend opens the
 * run `outputDir` (or the client-provided fallback) rather than revealing a file.
 */
export interface OpenYouTubeDownloadFolderRequest {
  runId: string | null;
  /** Optional occurrence id for future per-item reveal; unused in V1. */
  occurrenceId: string | null;
  /** Fallback when the durable run record is unavailable (preview / mock / lost run). */
  outputDir: string;
}

export interface OpenYouTubeDownloadFolderResponse {
  path: string;
}

export interface YouTubeHistoryEntry {
  runId: string;
  state: string;
  title: string;
  sourceUrl: string;
  videoCount: number;
  outputDir: string;
  createdAt: number;
  completedAt: number | null;
  errorMessage: string | null;
}

export interface ListYouTubeHistoryRequest {
  limit?: number;
}

export interface YouTubeProgressItem {
  occurrenceId: string;
  artifactFingerprint: string;
  videoId: string;
  ordinal: number;
  title: string;
  state: YouTubeItemState;
  phase: YouTubeItemPhase;
}

export interface YouTubeProgressCounts {
  completed: number;
  completedWithWarnings: number;
  selected: number;
  failed: number;
  skipped: number;
  cancelled: number;
}

export interface YouTubeProgressWarning {
  occurrenceId: string | null;
  code: YouTubeWarningCode;
  message: string;
}

export interface YouTubeProgressEvent {
  schemaVersion: 1;
  runId: string;
  revision: number;
  state: YouTubeRunState;
  item: YouTubeProgressItem | null;
  progress: {
    bytesCompleted: number | null;
    bytesTotal: number | null;
    fraction: number | null;
  };
  counts: YouTubeProgressCounts;
  warnings: YouTubeProgressWarning[];
  error: YouTubeError | null;
}

export interface YouTubeItemOutcomeSnapshot {
  occurrenceId: string;
  artifactFingerprint: string;
  videoId: string;
  ordinal: number;
  title: string;
  state: YouTubeItemState;
  phase: YouTubeItemPhase;
  /** Matches Rust `TransientWarning` objects on item outcomes (not bare string codes). */
  warnings: YouTubeProgressWarning[];
  error: YouTubeError | null;
  publishedArtifactKinds: Array<"media" | "vtt" | "transcript_json" | "metadata">;
}

export interface YouTubeRunSnapshot extends YouTubeProgressEvent {
  clientSubmissionId: string;
  planFingerprint: string;
  items: YouTubeItemOutcomeSnapshot[];
}

export function isYouTubeRunTerminal(state: YouTubeRunState | null | undefined): boolean {
  return state === "completed"
    || state === "completed_with_warnings"
    || state === "failed"
    || state === "cancelled";
}

export interface ListYouTubeRunVideosRequest {
  runId: string;
}

/**
 * One video of a finished run, as the in-app player needs it.
 * `mediaUrl` / `subtitlesUrl` are opaque `youtube-media` URLs (ids only).
 * An unavailable video has no URLs and says why in `unavailableReason`.
 */
export interface YouTubeRunVideo {
  occurrenceId: string;
  ordinal: number;
  title: string;
  videoId: string;
  mediaUrl: string | null;
  subtitlesUrl: string | null;
  available: boolean;
  unavailableReason: string | null;
  positionSeconds: number | null;
  durationSeconds: number | null;
  completed: boolean;
}

export interface GetYouTubePlaybackProgressRequest {
  runId: string;
  occurrenceId: string;
}

export interface SaveYouTubePlaybackProgressRequest {
  runId: string;
  occurrenceId: string;
  positionSeconds: number;
  durationSeconds: number;
}

export interface YouTubePlaybackProgress {
  positionSeconds: number;
  durationSeconds: number;
  completed: boolean;
}

/** What the history page hands the player: one run and the video to start on. */
export interface YouTubePlayerSession {
  runId: string;
  title: string;
  videos: YouTubeRunVideo[];
  startOccurrenceId: string;
}

/**
 * A live snapshot that ended because a resumed attempt failed: the run is
 * paused for the user (see `YouTubeResumableRun`), not running and not
 * finished. It carries the reason in `error`.
 */
export function isYouTubeRunSuspended(
  snapshot: { state: YouTubeRunState; error: YouTubeError | null } | null | undefined
): boolean {
  return snapshot?.state === "paused" && snapshot.error !== null;
}

/** Who moves an interrupted run next. */
export type YouTubeResumeState = "auto_pending" | "needs_user" | "resuming";

/** A run that was interrupted, or is in a resumed attempt (Rust `YouTubeResumableRun`). */
export interface YouTubeResumableRun {
  runId: string;
  state: YouTubeResumeState;
  reason: string | null;
  reasonCode: string | null;
  title: string;
  videoCount: number;
  outputDir: string;
  autoResumeUsed: boolean;
  /** Bytes of partial download a resume keeps. */
  preservedBytes: number;
  updatedAt: number;
  /** Identifies this pause; Continue sends it back so a stale click starts nothing. */
  generation: number;
}

export interface ContinueYouTubeRunRequest {
  runId: string;
  /** `YouTubeResumableRun.generation` of the pause the user is answering. */
  generation: number;
}

export interface ContinueYouTubeRunResponse {
  /** False when nothing was started because the run was already resuming or finished. */
  started: boolean;
}
