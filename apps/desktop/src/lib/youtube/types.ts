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
  | "completed"
  | "warning"
  | "failed"
  | "cancelled";

export type YouTubeWarningCode =
  | "PLAYLIST_TRUNCATED"
  | "TRANSCRIPT_FALLBACK_USED"
  | "TRANSCRIPT_MISSING"
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
  truncated: boolean;
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
  warnings: YouTubeWarningCode[];
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
