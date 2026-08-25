import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type FocusEvent, type KeyboardEvent, type PointerEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  Folder,
  FolderOpen,
  Pause,
  Play,
  Square
} from "lucide-react";
import {
  Button,
  Dialog,
  Progress,
  Select,
  Textarea
} from "../primitives";
import {
  cancelYouTubeDownload,
  getYouTubeDownloadState,
  getYouTubeHelperStatus,
  inspectYouTubeTranscripts,
  isTauriRuntime,
  listYouTubeHistory,
  openYouTubeDownloadFolder,
  pauseYouTubeDownload,
  readActiveYouTubePreviewScan,
  resumeYouTubeDownload,
  formatYouTubeInvokeError,
  scanYouTubeSource,
  startYouTubeDownload,
  subscribeYouTubeRunChanged,
  youtubeErrorFromUnknown,
  isYouTubeRunSnapshot,
  YOUTUBE_UI_MOCK_RUN_PREFIX
} from "../../lib/youtube/ipc";
import {
  detectYouTubeLinks,
  detectedKindLabel,
  firstCompleteYouTubeLink,
  isAmbiguousWatchPlaylist,
  type DetectedYouTubeLink
} from "../../lib/youtube/detect";
import {
  isYouTubeRunTerminal,
  type InspectYouTubeTranscriptsResponse,
  type ScanYouTubeSourceResponse,
  type StartYouTubeDownloadRequest,
  type YouTubeDownloadMode,
  type YouTubeError,
  type YouTubeHistoryEntry,
  type YouTubeItemState,
  type YouTubePlaylistMode,
  type YouTubeRunSnapshot,
  type YouTubeScanItem,
  type YouTubeTranscriptTrack
} from "../../lib/youtube/types";
import { loadYouTubeOutputDir, persistYouTubeOutputDir, readPreviewYouTubeOutputDir } from "../../lib/youtube/preferences";

type HelperStatus = "pending" | "ready" | "failed";
type YouTubeQueueSection = "queue" | "active" | "completed" | "failed";

function queueSectionForItemState(state: YouTubeItemState): YouTubeQueueSection {
  switch (state) {
    case "running":
      return "active";
    case "completed":
    case "completed_with_warnings":
    case "skipped_existing":
      return "completed";
    case "failed":
    case "cancelled":
      return "failed";
    case "pending":
    case "skipped":
    default:
      return "queue";
  }
}

function YouTubeQueueSectionTab({
  section,
  label,
  value,
  tone,
  selected,
  onClick
}: {
  section: YouTubeQueueSection;
  label: string;
  value: number;
  tone: "queue" | "primary" | "success" | "danger";
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`queue-section-tab${selected ? " is-selected" : ""}`}
      data-section={section}
      data-tone={tone}
      aria-pressed={selected}
      onClick={onClick}
    >
      <span className="queue-section-tab-label">
        <span className="queue-section-tab-dot" aria-hidden="true" />
        {label}
      </span>
      <strong>{value}</strong>
    </button>
  );
}

interface ResultVideo {
  scanPlanId: string;
  item: YouTubeScanItem;
}

interface DownloadGroup {
  scanPlanId: string;
  occurrenceIds: string[];
}

interface TranscriptLanguageOption {
  tag: string;
  label: string;
}

const NO_CAPTION_OPTION: TranscriptLanguageOption = { tag: "", label: "No caption" };
/** Matches `.youtube-search-input` min/max-height; keep in sync with CSS. */
const YOUTUBE_SEARCH_MIN_HEIGHT_PX = 40;
const YOUTUBE_SEARCH_MAX_HEIGHT_PX = 132;
/** Cap post-scan caption probes so large playlists stay usable (spec NFR-6). */
const YOUTUBE_LANGUAGE_PROBE_LIMIT = 3;
/** Debounce typed URL changes; paste still scans immediately. */
const YOUTUBE_AUTO_SCAN_DEBOUNCE_MS = 400;
/** Empty-scan skeleton count; keep layout stable before the first result arrives. */
const YOUTUBE_SCAN_SKELETON_COUNT = 3;

function YouTubeScanSkeletonRows({ count }: { count: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, index) => (
        <li
          key={`youtube-scan-skeleton-${index}`}
          className="youtube-result-row youtube-result-row-skeleton"
          aria-hidden="true"
        >
          <div className="youtube-result-copy">
            <span className="youtube-skeleton-line youtube-skeleton-title" />
            <span className="youtube-skeleton-line youtube-skeleton-meta" />
          </div>
          <div className="youtube-result-overlay">
            <span className="youtube-skeleton-line youtube-skeleton-action" />
          </div>
        </li>
      ))}
    </>
  );
}

function groupsForVideos(targets: ResultVideo[]): DownloadGroup[] {
  const groups: DownloadGroup[] = [];
  for (const target of targets) {
    if (target.item.availability !== "available") continue;
    const existing = groups.find((group) => group.scanPlanId === target.scanPlanId);
    if (existing) existing.occurrenceIds.push(target.item.occurrenceId);
    else groups.push({ scanPlanId: target.scanPlanId, occurrenceIds: [target.item.occurrenceId] });
  }
  return groups;
}

function collectLanguageOptions(tracks: YouTubeTranscriptTrack[]): TranscriptLanguageOption[] {
  const options: TranscriptLanguageOption[] = [];
  for (const track of tracks) {
    const tag = track.languageTag.trim();
    if (!tag) continue;
    const existing = options.find((option) => option.tag.toLowerCase() === tag.toLowerCase());
    if (existing) {
      if (track.source === "uploader" && !existing.label.toLowerCase().includes("automatic")) {
        existing.label = track.displayLanguage || existing.label;
      }
      continue;
    }
    options.push({
      tag,
      label: track.source === "automatic" && !track.displayLanguage.toLowerCase().includes("automatic")
        ? `${track.displayLanguage} (automatic)`
        : track.displayLanguage || tag
    });
  }
  if (options.length === 0) return [NO_CAPTION_OPTION];
  options.sort((left, right) => left.label.localeCompare(right.label));
  const englishIndex = options.findIndex((option) => {
    const tag = option.tag.toLowerCase();
    return tag === "en" || tag.startsWith("en-") || tag.startsWith("en_");
  });
  if (englishIndex > 0) {
    const [english] = options.splice(englishIndex, 1);
    if (english) options.unshift(english);
  }
  return options;
}

function pickPreferredLanguage(
  options: TranscriptLanguageOption[],
  current: string | null
): string | null {
  if (options.length === 0 || (options.length === 1 && options[0]?.tag === "")) return null;
  if (current && options.some((option) => option.tag === current)) return current;
  const english = options.find((option) => {
    const tag = option.tag.toLowerCase();
    return tag === "en" || tag.startsWith("en-") || tag.startsWith("en_");
  });
  return english?.tag ?? options[0]?.tag ?? null;
}

function helperStatusFailure(error: unknown): YouTubeError {
  return youtubeErrorFromUnknown(error);
}

function createClientId(prefix: string): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0"));
    return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10, 16).join("")}`;
  }
  return `${prefix}-${Date.now()}-${Math.floor(Math.random() * 1_000_000)}`;
}

function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "";
  const total = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(total / 60);
  const remainder = total % 60;
  return `${minutes}:${String(remainder).padStart(2, "0")}`;
}

function completeLinkFingerprint(links: DetectedYouTubeLink[]): string {
  return links
    .filter((link) => link.complete)
    .map((link) => link.canonicalUrl)
    .sort()
    .join("\n");
}

function itemProgressPercent(
  occurrenceId: string,
  state: YouTubeItemState,
  runSnapshot: YouTubeRunSnapshot | null,
  runPercent: number
): number | null {
  if (state === "completed" || state === "skipped_existing") return 100;
  if (state === "running" && runSnapshot?.item?.occurrenceId === occurrenceId) return runPercent;
  if (state === "pending" && runSnapshot && !isYouTubeRunTerminal(runSnapshot.state)) return 0;
  return null;
}

function itemStatusText(state: YouTubeItemState, available: boolean): string | null {
  if (!available) return state === "skipped" ? "Unavailable" : "Unconfirmed";
  switch (state) {
    case "completed": return "Saved";
    case "completed_with_warnings": return "Saved with warnings";
    case "running": return "Downloading";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    case "skipped_existing": return "Already saved";
    default: return null;
  }
}

function readTrimmedString(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/** Accepts string codes or Rust `{ code, message }` warning objects from IPC. */
function formatYouTubeWarning(value: unknown): string {
  let code: string | null = null;
  let message: string | null = null;
  if (typeof value === "string") {
    code = readTrimmedString(value);
  } else if (typeof value === "object" && value !== null) {
    const record = value as { code?: unknown; message?: unknown };
    code = readTrimmedString(record.code);
    message = readTrimmedString(record.message);
  }

  switch (code) {
    case "TRANSCRIPT_MISSING":
      return "No captions were available on YouTube for this video, so only the media file was saved.";
    default:
      if (message && message !== code) return message;
      return code ?? "Completed with warnings.";
  }
}

export function YouTubeView({ mode = "downloads" }: { mode?: "downloads" | "history" }) {
  const nativeRuntime = isTauriRuntime();
  const [helperStatus, setHelperStatus] = useState<HelperStatus>(nativeRuntime ? "pending" : "ready");
  const [helperError, setHelperError] = useState<YouTubeError | null>(null);
  const helperReady = !nativeRuntime || helperStatus === "ready";
  const [sourceUrl, setSourceUrl] = useState("");
  const [detectedLinks, setDetectedLinks] = useState<DetectedYouTubeLink[]>([]);
  const [scanPlans, setScanPlans] = useState<ScanYouTubeSourceResponse[]>([]);
  const [videos, setVideos] = useState<ResultVideo[]>([]);
  const [selectedOccurrenceIds, setSelectedOccurrenceIds] = useState<Set<string>>(() => new Set());
  const [outputDir, setOutputDir] = useState(() => (nativeRuntime ? "" : readPreviewYouTubeOutputDir()));
  const [folderGateOpen, setFolderGateOpen] = useState(false);
  const [playlistMode, setPlaylistMode] = useState<YouTubePlaylistMode>("video");
  const [downloadMode, setDownloadMode] = useState<YouTubeDownloadMode>("video_and_transcript");
  const [maxHeight, setMaxHeight] = useState<StartYouTubeDownloadRequest["maxHeight"]>(1080);
  const [preferredLanguage, setPreferredLanguage] = useState<StartYouTubeDownloadRequest["preferredLanguage"]>(null);
  const [languageOptions, setLanguageOptions] = useState<TranscriptLanguageOption[]>([NO_CAPTION_OPTION]);
  const [isScanning, setIsScanning] = useState(false);
  const [isDetectingLanguages, setIsDetectingLanguages] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isPausing, setIsPausing] = useState(false);
  const [isResuming, setIsResuming] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [runSnapshot, setRunSnapshot] = useState<YouTubeRunSnapshot | null>(null);
  const [queueSection, setQueueSection] = useState<YouTubeQueueSection>("queue");
  const [historyEntries, setHistoryEntries] = useState<YouTubeHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(mode === "history");
  const [historyError, setHistoryError] = useState<string | null>(null);
  const sourceInputRef = useRef<HTMLTextAreaElement | null>(null);
  const transcriptInspectionGenerationRef = useRef(0);
  const latestRunIdRef = useRef<string | null>(null);
  const latestRevisionRef = useRef(0);
  const scanGenerationRef = useRef(0);
  const languageProbeTokenRef = useRef(0);
  const playlistModeRef = useRef(playlistMode);
  playlistModeRef.current = playlistMode;
  const lastFingerprintRef = useRef("");
  const downloadQueueRef = useRef<DownloadGroup[]>([]);
  const startingGroupRef = useRef(false);
  const warnedRunIdRef = useRef<string | null>(null);
  const autoScanTimerRef = useRef<number | null>(null);
  const lastHeightSyncKeyRef = useRef<string | null>(null);
  /** Output dir used for the accepted run; File action falls back to this when paths are absent. */
  const runOutputDirRef = useRef("");
  const hasDestinationFolder = outputDir.trim().length > 0;
  const ambiguousPlaylistSource = detectedLinks.some((link) => link.kind === "ambiguous")
    || scanPlans.some((plan) => isAmbiguousWatchPlaylist(plan.canonicalUrl));
  const sourceNewlineCount = useMemo(
    () => (sourceUrl.match(/\n/g) ?? []).length,
    [sourceUrl]
  );
  const sourceIsEmpty = sourceUrl.trim().length === 0;

  const syncSearchInputHeight = useCallback((force = false) => {
    const el = sourceInputRef.current;
    if (!el) return;
    const syncKey = `${sourceNewlineCount}:${sourceIsEmpty ? "empty" : "filled"}`;
    if (!force && lastHeightSyncKeyRef.current === syncKey) return;
    lastHeightSyncKeyRef.current = syncKey;
    el.style.height = "0px";
    const next = Math.min(
      Math.max(el.scrollHeight, YOUTUBE_SEARCH_MIN_HEIGHT_PX),
      YOUTUBE_SEARCH_MAX_HEIGHT_PX
    );
    el.style.height = `${next}px`;
  }, [sourceNewlineCount, sourceIsEmpty]);

  useLayoutEffect(() => {
    syncSearchInputHeight();
  }, [syncSearchInputHeight]);

  useEffect(() => {
    return () => {
      if (autoScanTimerRef.current !== null) {
        window.clearTimeout(autoScanTimerRef.current);
        autoScanTimerRef.current = null;
      }
    };
  }, []);

  useEffect(() => {
    if (!nativeRuntime) return;
    let disposed = false;
    void getYouTubeHelperStatus()
      .then((response) => {
        if (disposed) return;
        const ready = response.status === "ready";
        setHelperStatus(ready ? "ready" : "failed");
        setHelperError(ready
          ? null
          : {
              code: response.code ?? "HELPER_INTEGRITY_FAILED",
              message: response.message || "YouTube helper integrity validation failed."
            });
      })
      .catch((error: unknown) => {
        if (disposed) return;
        setHelperStatus("failed");
        setHelperError(helperStatusFailure(error));
      });
    return () => {
      disposed = true;
    };
  }, [nativeRuntime]);

  useEffect(() => {
    if (!nativeRuntime) return;
    let disposed = false;
    void loadYouTubeOutputDir()
      .then((dir) => {
        if (disposed || !dir) return;
        setOutputDir(dir);
      })
      .catch((error: unknown) => {
        if (disposed) return;
        toast.error("Could not load YouTube folder preference", {
          description: formatYouTubeInvokeError(error)
        });
      });
    return () => {
      disposed = true;
    };
  }, [nativeRuntime]);

  useEffect(() => {
    if (mode !== "history") return;
    let disposed = false;
    setHistoryLoading(true);
    setHistoryError(null);
    void listYouTubeHistory()
      .then((entries) => {
        if (disposed) return;
        setHistoryEntries(entries);
      })
      .catch((error: unknown) => {
        if (disposed) return;
        setHistoryError(formatYouTubeInvokeError(error));
        setHistoryEntries([]);
      })
      .finally(() => {
        if (!disposed) setHistoryLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [mode]);

  const applyRunSnapshot = useCallback((snapshot: YouTubeRunSnapshot | null, allowRunSwitch = false) => {
    if (!snapshot) return;
    const currentRunId = latestRunIdRef.current;
    const sameRun = currentRunId === snapshot.runId;
    if (currentRunId !== null && !sameRun && !allowRunSwitch) return;
    if (sameRun && snapshot.revision <= latestRevisionRef.current) return;
    latestRunIdRef.current = snapshot.runId;
    latestRevisionRef.current = snapshot.revision;
    setRunSnapshot(snapshot);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        // Subscribe before the state request so a revision cannot land between them.
        const cleanup = await subscribeYouTubeRunChanged((event) => {
          if (disposed) return;
          const currentRunId = latestRunIdRef.current;
          const mockRun = event.runId.startsWith(YOUTUBE_UI_MOCK_RUN_PREFIX);
          if (currentRunId !== null && currentRunId !== event.runId && !mockRun) return;
          // Live/preview emit full snapshots; apply them directly to avoid a refetch per revision.
          if (isYouTubeRunSnapshot(event)) {
            applyRunSnapshot(event, mockRun);
            return;
          }
          void getYouTubeDownloadState({ runId: event.runId })
            .then((snapshot) => {
              if (!disposed) applyRunSnapshot(snapshot, mockRun);
            })
            .catch(() => undefined);
        });
        if (disposed) cleanup();
        else unlisten = cleanup;
        const snapshot = await getYouTubeDownloadState({ runId: null });
        if (!disposed) applyRunSnapshot(snapshot, true);
      } catch (error) {
        if (!disposed && nativeRuntime) {
          toast.error("YouTube runtime state unavailable", { description: formatYouTubeInvokeError(error) });
        }
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyRunSnapshot, nativeRuntime]);

  useEffect(() => {
    if (!runSnapshot?.runId.startsWith(YOUTUBE_UI_MOCK_RUN_PREFIX)) return;
    if (!runOutputDirRef.current.trim()) {
      runOutputDirRef.current = outputDir.trim() || "C:\\Users\\Public\\Videos";
    }
    const scan = readActiveYouTubePreviewScan();
    if (scan) {
      setScanPlans([scan]);
      setVideos(scan.items.map((item) => ({ scanPlanId: scan.scanPlanId, item })));
      setSelectedOccurrenceIds(new Set(scan.items.map((item) => item.occurrenceId)));
      setSourceUrl(scan.canonicalUrl);
      return;
    }
    setVideos(runSnapshot.items.map((item) => ({
      scanPlanId: "youtube-ui-mock",
      item: {
        occurrenceId: item.occurrenceId,
        videoId: item.videoId,
        sourceUrl: `https://www.youtube.com/watch?v=${item.videoId}`,
        title: item.title,
        ordinal: item.ordinal,
        channelName: null,
        channelId: null,
        durationSeconds: null,
        thumbnailAvailable: false,
        availability: "available" as const,
        metadataDigest: item.artifactFingerprint
      }
    })));
  }, [runSnapshot?.runId, outputDir]);

  const availableVideos = useMemo(
    () => videos.filter((video) => video.item.availability === "available"),
    [videos]
  );
  const runItems = useMemo(
    () => new Map((runSnapshot?.items ?? []).map((item) => [item.occurrenceId, item])),
    [runSnapshot]
  );
  const videosWithSection = useMemo(
    () => videos.map((video) => {
      const outcome = runItems.get(video.item.occurrenceId);
      const available = video.item.availability === "available";
      const state: YouTubeItemState = outcome?.state ?? (available ? "pending" : "skipped");
      return { video, state, section: queueSectionForItemState(state) };
    }),
    [videos, runItems]
  );
  const queueCounts = useMemo(() => {
    const counts = { queue: 0, active: 0, completed: 0, failed: 0 };
    for (const entry of videosWithSection) {
      counts[entry.section] += 1;
    }
    return counts;
  }, [videosWithSection]);
  const sectionVideos = useMemo(
    () => videosWithSection.filter((entry) => entry.section === queueSection),
    [videosWithSection, queueSection]
  );
  const activeRun = runSnapshot !== null && !isYouTubeRunTerminal(runSnapshot.state);
  const canPauseRun = activeRun && runSnapshot?.state === "running";
  const canResumeRun = activeRun && (runSnapshot?.state === "paused" || runSnapshot?.state === "pause_requested");
  const currentProgress = runSnapshot?.progress.fraction === null || runSnapshot?.progress.fraction === undefined
    ? 0
    : Math.max(0, Math.min(1, runSnapshot.progress.fraction)) * 100;
  const multipleResults = videos.length > 1;
  const showQueueDownloads = queueSection === "queue" && !activeRun;
  const fallbackLanguages = useMemo(
    () => languageOptions
      .map((option) => option.tag)
      .filter((tag) => tag !== preferredLanguage)
      .slice(0, 8),
    [languageOptions, preferredLanguage]
  );
  const liveAnnouncement = isScanning && videos.length === 0
    ? "Finding videos…"
    : runSnapshot
      ? `${runSnapshot.state}. ${runSnapshot.counts.completed} of ${runSnapshot.counts.selected} complete${runSnapshot.item ? `. ${runSnapshot.item.title}` : ""}.`
      : videos.length > 0
        ? `${videos.length} video${videos.length === 1 ? "" : "s"} detected.`
        : "Paste a YouTube link to detect videos.";

  useEffect(() => {
    if (!runSnapshot || runSnapshot.state !== "completed_with_warnings") return;
    if (warnedRunIdRef.current === runSnapshot.runId) return;
    warnedRunIdRef.current = runSnapshot.runId;
    const warnings = [
      ...runSnapshot.warnings,
      ...runSnapshot.items.flatMap((item) => item.warnings)
    ];
    const first = warnings.find((warning) => warning.code === "TRANSCRIPT_MISSING") ?? warnings[0] ?? null;
    if (!first) return;
    toast.warning("Saved with warnings", { description: formatYouTubeWarning(first) });
  }, [runSnapshot]);

  function resetDetectedResults(): void {
    transcriptInspectionGenerationRef.current += 1;
    scanGenerationRef.current += 1;
    languageProbeTokenRef.current += 1;
    lastFingerprintRef.current = "";
    setScanPlans([]);
    setVideos([]);
    setSelectedOccurrenceIds(new Set());
    setLanguageOptions([NO_CAPTION_OPTION]);
    setPreferredLanguage(null);
    setIsDetectingLanguages(false);
    downloadQueueRef.current = [];
  }

  function applySourceText(next: string): DetectedYouTubeLink[] {
    const links = detectYouTubeLinks(next);
    setSourceUrl(next);
    setDetectedLinks(links);
    if (!next.trim()) {
      resetDetectedResults();
    }
    return links;
  }

  async function refreshDetectedLanguages(
    nextVideos: ResultVideo[],
    generation: number,
    occurrenceLimit = YOUTUBE_LANGUAGE_PROBE_LIMIT
  ): Promise<void> {
    const groups = groupsForVideos(nextVideos);
    if (groups.length === 0 || occurrenceLimit <= 0) {
      setLanguageOptions([NO_CAPTION_OPTION]);
      setPreferredLanguage(null);
      return;
    }
    let remaining = occurrenceLimit;
    const limitedGroups: DownloadGroup[] = [];
    for (const group of groups) {
      if (remaining <= 0) break;
      const occurrenceIds = group.occurrenceIds.slice(0, remaining);
      if (occurrenceIds.length === 0) continue;
      limitedGroups.push({ scanPlanId: group.scanPlanId, occurrenceIds });
      remaining -= occurrenceIds.length;
    }
    if (limitedGroups.length === 0) {
      setLanguageOptions([NO_CAPTION_OPTION]);
      setPreferredLanguage(null);
      return;
    }
    const probeToken = ++languageProbeTokenRef.current;
    setIsDetectingLanguages(true);
    const tracks: YouTubeTranscriptTrack[] = [];
    try {
      for (const group of limitedGroups) {
        if (generation !== scanGenerationRef.current) return;
        try {
          const response = await inspectYouTubeTranscripts({
            clientOperationId: createClientId("youtube-transcript-operation"),
            scanPlanId: group.scanPlanId,
            occurrenceIds: group.occurrenceIds
          });
          if (generation !== scanGenerationRef.current) return;
          for (const occurrence of response.occurrences) {
            tracks.push(...occurrence.tracks);
          }
        } catch {
          continue;
        }
      }
      if (generation !== scanGenerationRef.current) return;
      const options = collectLanguageOptions(tracks);
      setLanguageOptions(options);
      setPreferredLanguage((current) => pickPreferredLanguage(options, current));
    } finally {
      // Only the latest probe may clear the flag; an aborted probe must not
      // clobber a newer in-flight detect (which would re-enable Language mid-work).
      if (probeToken === languageProbeTokenRef.current) {
        setIsDetectingLanguages(false);
      }
    }
  }

  function ensureLanguageOptionsForSelection(): void {
    if (isDetectingLanguages || activeRun) return;
    if (languageOptions.some((option) => option.tag !== "")) return;
    const selected = videos.filter((video) => selectedOccurrenceIds.has(video.item.occurrenceId));
    const targets = selected.length > 0 ? selected : availableVideos;
    if (targets.length === 0) return;
    const generation = scanGenerationRef.current;
    void refreshDetectedLanguages(targets, generation, YOUTUBE_LANGUAGE_PROBE_LIMIT);
  }

  async function handleScan(urlOverride?: string, nextPlaylistMode?: YouTubePlaylistMode): Promise<void> {
    const raw = (urlOverride ?? sourceUrl).trim();
    const links = detectYouTubeLinks(raw);
    const complete = links.filter((link) => link.complete);
    const fallback = complete.length > 0 ? complete : links.filter((link) => Boolean(firstCompleteYouTubeLink(link.canonicalUrl)));
    await scanCompleteLinks(fallback.length > 0 ? fallback : complete, nextPlaylistMode);
  }

  async function scanCompleteLinks(
    links: DetectedYouTubeLink[],
    nextPlaylistMode?: YouTubePlaylistMode
  ): Promise<void> {
    if (!ensureDestinationFolder()) return;
    const unique = links.filter((link, index) => links.findIndex((candidate) => candidate.canonicalUrl === link.canonicalUrl) === index);
    const complete = unique.filter((link) => link.complete);
    if (complete.length === 0) {
      toast.error("Enter a YouTube URL first");
      return;
    }
    if (!helperReady) {
      toast.error("YouTube helper is not ready", { description: helperError?.message ?? "Native discovery is blocked until helper integrity passes." });
      return;
    }
    const fingerprint = completeLinkFingerprint(complete);
    lastFingerprintRef.current = fingerprint;
    transcriptInspectionGenerationRef.current += 1;
    languageProbeTokenRef.current += 1;
    setScanPlans([]);
    setVideos([]);
    setSelectedOccurrenceIds(new Set());
    setLanguageOptions([NO_CAPTION_OPTION]);
    setPreferredLanguage(null);
    setIsDetectingLanguages(false);
    setIsScanning(true);
    setQueueSection("queue");
    const generation = ++scanGenerationRef.current;
    const mergedPlans: ScanYouTubeSourceResponse[] = [];
    const mergedVideos: ResultVideo[] = [];
    const seenVideoIds = new Set<string>();
    try {
      for (const link of complete) {
        const resolvedPlaylistMode = nextPlaylistMode ?? playlistModeRef.current;
        const nextScan = await scanYouTubeSource({
          clientOperationId: createClientId("youtube-operation"),
          url: link.canonicalUrl,
          playlistMode: isAmbiguousWatchPlaylist(link.canonicalUrl) ? resolvedPlaylistMode : undefined
        });
        if (generation !== scanGenerationRef.current) return;
        transcriptInspectionGenerationRef.current += 1;
        mergedPlans.push(nextScan);
        for (const item of nextScan.items) {
          if (seenVideoIds.has(item.videoId)) continue;
          seenVideoIds.add(item.videoId);
          mergedVideos.push({ scanPlanId: nextScan.scanPlanId, item });
        }
        setScanPlans([...mergedPlans]);
        setVideos([...mergedVideos]);
        const available = mergedVideos
          .filter((video) => video.item.availability === "available")
          .map((video) => video.item);
        setSelectedOccurrenceIds(new Set(available.map((item) => item.occurrenceId)));
      }
      if (generation === scanGenerationRef.current) {
        // Probe a bounded sample only — never full-playlist caption inspection after scan.
        await refreshDetectedLanguages(mergedVideos, generation, YOUTUBE_LANGUAGE_PROBE_LIMIT);
      }
    } catch (error) {
      if (generation !== scanGenerationRef.current) return;
      toast.error("YouTube scan failed", { description: formatYouTubeInvokeError(error) });
    } finally {
      if (generation === scanGenerationRef.current) setIsScanning(false);
    }
  }

  function requestAutoScan(links: DetectedYouTubeLink[]): void {
    const fingerprint = completeLinkFingerprint(links);
    if (!fingerprint) return;
    if (fingerprint === lastFingerprintRef.current) return;
    void scanCompleteLinks(links.filter((link) => link.complete));
  }

  function scheduleAutoScan(links: DetectedYouTubeLink[]): void {
    if (autoScanTimerRef.current !== null) {
      window.clearTimeout(autoScanTimerRef.current);
    }
    autoScanTimerRef.current = window.setTimeout(() => {
      autoScanTimerRef.current = null;
      requestAutoScan(links);
    }, YOUTUBE_AUTO_SCAN_DEBOUNCE_MS);
  }

  function openDestinationFolderGate(): void {
    setFolderGateOpen(true);
  }

  function ensureDestinationFolder(): boolean {
    if (hasDestinationFolder) return true;
    openDestinationFolderGate();
    return false;
  }

  function handleSourcePointerDown(event: PointerEvent<HTMLTextAreaElement>): void {
    if (hasDestinationFolder) return;
    event.preventDefault();
    openDestinationFolderGate();
  }

  function handleSourceFocus(event: FocusEvent<HTMLTextAreaElement>): void {
    if (hasDestinationFolder) return;
    event.currentTarget.blur();
    openDestinationFolderGate();
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>): void {
    if (!ensureDestinationFolder()) {
      event.preventDefault();
      return;
    }
    const pasted = event.clipboardData.getData("text");
    if (!pasted) return;
    const target = event.currentTarget;
    const start = target.selectionStart ?? sourceUrl.length;
    const end = target.selectionEnd ?? sourceUrl.length;
    const replacingAll = start === 0 && end >= sourceUrl.length;
    const next = replacingAll || !sourceUrl.trim()
      ? pasted
      : `${sourceUrl.slice(0, start)}${pasted}${sourceUrl.slice(end)}`;
    event.preventDefault();
    if (autoScanTimerRef.current !== null) {
      window.clearTimeout(autoScanTimerRef.current);
      autoScanTimerRef.current = null;
    }
    const links = detectYouTubeLinks(next);
    setSourceUrl(next);
    setDetectedLinks(links);
    window.setTimeout(() => syncSearchInputHeight(true), 0);
    if (!next.trim()) {
      resetDetectedResults();
      return;
    }
    const complete = links.find((link) => link.complete) ?? null;
    if (!complete) return;
    requestAutoScan(links);
  }

  function handleSourceChange(next: string): void {
    if (!ensureDestinationFolder()) return;
    const links = applySourceText(next);
    if (!next.trim()) {
      if (autoScanTimerRef.current !== null) {
        window.clearTimeout(autoScanTimerRef.current);
        autoScanTimerRef.current = null;
      }
      return;
    }
    scheduleAutoScan(links);
  }

  function handleSearchKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (!ensureDestinationFolder()) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      if (autoScanTimerRef.current !== null) {
        window.clearTimeout(autoScanTimerRef.current);
        autoScanTimerRef.current = null;
      }
      void handleScan();
    }
  }

  async function requestTranscriptInspection(
    scanPlanId: string,
    occurrenceIds: string[]
  ): Promise<InspectYouTubeTranscriptsResponse | null> {
    if (occurrenceIds.length === 0 || !helperReady) return null;
    const requestGeneration = transcriptInspectionGenerationRef.current + 1;
    transcriptInspectionGenerationRef.current = requestGeneration;
    try {
      const response = await inspectYouTubeTranscripts({
        clientOperationId: createClientId("youtube-transcript-operation"),
        scanPlanId,
        occurrenceIds
      });
      if (requestGeneration !== transcriptInspectionGenerationRef.current) return null;
      const requestedOccurrenceIds = occurrenceIds;
      const responseMatchesRequest = response.occurrences.length === requestedOccurrenceIds.length
        && response.occurrences.every((occurrence, index) => occurrence.occurrenceId === requestedOccurrenceIds[index]);
      if (!responseMatchesRequest) throw new Error("Transcript inspection response did not match the current occurrence selection.");
      return response;
    } catch (error) {
      if (requestGeneration !== transcriptInspectionGenerationRef.current) return null;
      toast.error("Transcript inspection failed", { description: formatYouTubeInvokeError(error) });
      return null;
    }
  }

  async function pickOutputDirectory(): Promise<string | null> {
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: outputDir || undefined });
      if (typeof picked === "string" && picked.trim()) {
        const nextOutputDir = await persistYouTubeOutputDir(picked.trim());
        setOutputDir(nextOutputDir);
        return nextOutputDir;
      }
      return null;
    } catch (error) {
      toast.error("Folder picker failed", { description: formatYouTubeInvokeError(error) });
      return null;
    }
  }

  async function confirmDestinationFolderFromGate(): Promise<void> {
    const picked = await pickOutputDirectory();
    if (!picked) return;
    setFolderGateOpen(false);
    window.setTimeout(() => sourceInputRef.current?.focus(), 0);
  }

  async function startGroup(group: DownloadGroup): Promise<boolean> {
    const scan = scanPlans.find((plan) => plan.scanPlanId === group.scanPlanId) ?? null;
    if (!scan || group.occurrenceIds.length === 0) {
      toast.error("Choose at least one public occurrence");
      return false;
    }
    if (!helperReady) return false;
    setSelectedOccurrenceIds(new Set(group.occurrenceIds));
    startingGroupRef.current = true;
    setIsStarting(true);
    try {
      const resolvedOutputDir = outputDir.trim() || await pickOutputDirectory();
      if (!resolvedOutputDir) return false;
      if (downloadMode !== "video_only") {
        const inspected = await requestTranscriptInspection(group.scanPlanId, group.occurrenceIds);
        if (!inspected) return false;
        const missingCaptions = inspected.occurrences.some((occurrence) => occurrence.tracks.length === 0);
        if (missingCaptions && downloadMode === "video_and_transcript") {
          toast.message("No captions on YouTube", {
            description: "This video has no uploader or automatic captions. The download will continue and save the video only."
          });
        }
      }
      const response = await startYouTubeDownload({
        clientSubmissionId: createClientId("youtube-submission"),
        scanPlanId: scan.scanPlanId,
        selectedOccurrenceIds: group.occurrenceIds,
        outputDir: resolvedOutputDir,
        mode: downloadMode,
        maxHeight,
        preferredLanguage,
        fallbackLanguages,
        allowAutomaticCaptions: true,
        continueWithoutTranscript: true
      });
      latestRunIdRef.current = response.receipt.runId;
      latestRevisionRef.current = 0;
      runOutputDirRef.current = resolvedOutputDir;
      setQueueSection("active");
      setRunSnapshot(null);
      const snapshot = await getYouTubeDownloadState({ runId: response.receipt.runId });
      applyRunSnapshot(snapshot, true);
      return true;
    } catch (error) {
      toast.error("YouTube download could not start", { description: formatYouTubeInvokeError(error) });
      return false;
    } finally {
      startingGroupRef.current = false;
      setIsStarting(false);
    }
  }

  async function startDownloads(targets: ResultVideo[]): Promise<void> {
    const groups = groupsForVideos(targets);
    if (groups.length === 0) {
      toast.error("Choose at least one public occurrence");
      return;
    }
    const firstGroup = groups[0];
    if (!firstGroup) return;
    downloadQueueRef.current = groups.slice(1);
    const started = await startGroup(firstGroup);
    if (!started) downloadQueueRef.current = [];
  }

  const startGroupRef = useRef(startGroup);
  startGroupRef.current = startGroup;

  useEffect(() => {
    if (!runSnapshot || !isYouTubeRunTerminal(runSnapshot.state) || startingGroupRef.current) return;
    const next = downloadQueueRef.current.shift();
    if (!next) return;
    void startGroupRef.current(next);
  }, [runSnapshot]);

  async function handleStart(): Promise<void> {
    await startDownloads(videos.filter((video) => selectedOccurrenceIds.has(video.item.occurrenceId)));
  }

  async function handleDownloadOne(video: ResultVideo): Promise<void> {
    downloadQueueRef.current = [];
    await startDownloads([video]);
  }

  async function handleCancel(): Promise<void> {
    if (!runSnapshot || !activeRun) return;
    downloadQueueRef.current = [];
    setIsCancelling(true);
    try {
      const snapshot = await cancelYouTubeDownload({ runId: runSnapshot.runId });
      applyRunSnapshot(snapshot);
    } catch (error) {
      toast.error("Could not cancel YouTube run", { description: formatYouTubeInvokeError(error) });
    } finally {
      setIsCancelling(false);
    }
  }

  async function handlePause(): Promise<void> {
    if (!runSnapshot || !canPauseRun) return;
    setIsPausing(true);
    try {
      const snapshot = await pauseYouTubeDownload({ runId: runSnapshot.runId, expectedRevision: runSnapshot.revision });
      applyRunSnapshot(snapshot);
    } catch (error) {
      toast.error("Could not pause YouTube run", { description: formatYouTubeInvokeError(error) });
    } finally {
      setIsPausing(false);
    }
  }

  async function handleResume(): Promise<void> {
    if (!runSnapshot || !canResumeRun) return;
    setIsResuming(true);
    try {
      const snapshot = await resumeYouTubeDownload({ runId: runSnapshot.runId, expectedRevision: runSnapshot.revision });
      applyRunSnapshot(snapshot);
    } catch (error) {
      toast.error("Could not resume YouTube run", { description: formatYouTubeInvokeError(error) });
    } finally {
      setIsResuming(false);
    }
  }

  async function openCompletedOccurrenceFolder(occurrenceId: string): Promise<void> {
    const fallbackPath = (runOutputDirRef.current.trim() || outputDir).trim();
    if (!fallbackPath) {
      toast.warning("Folder unavailable", {
        description: "Choose a download folder before opening completed files."
      });
      return;
    }
    try {
      // V1 item snapshots omit per-occurrence media paths; open the run outputDir.
      const opened = await openYouTubeDownloadFolder({
        runId: runSnapshot?.runId ?? null,
        occurrenceId,
        outputDir: fallbackPath
      });
      if (isTauriRuntime() && !(runSnapshot?.runId.startsWith(YOUTUBE_UI_MOCK_RUN_PREFIX))) {
        toast.success("Folder opened", { description: opened.path });
      } else {
        toast.info("Folder opener is only available in the Tauri desktop runtime", {
          description: opened.path
        });
      }
    } catch (error) {
      toast.error("Open folder failed", { description: formatYouTubeInvokeError(error) });
    }
  }

  function handlePlaylistModeChange(nextMode: YouTubePlaylistMode): void {
    setPlaylistMode(nextMode);
    const complete = detectedLinks.filter((link) => link.complete);
    if (complete.length > 0) {
      lastFingerprintRef.current = "";
      void scanCompleteLinks(complete, nextMode);
    }
  }

  function renderRunControls() {
    if (activeRun) {
      return (
        <>
          <Button type="button" size="xs" variant="outline" onClick={() => void (canResumeRun ? handleResume() : handlePause())} loading={isPausing || isResuming} loadingLabel={canResumeRun ? "Resuming" : "Pausing"} disabled={(!canPauseRun && !canResumeRun) || isCancelling}>
            {canResumeRun ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
            {canResumeRun ? "Resume" : "Pause"}
          </Button>
          <Button type="button" size="xs" variant="outline" onClick={() => void handleCancel()} loading={isCancelling} loadingLabel="Cancelling" disabled={!activeRun}>
            <Square aria-hidden="true" />
            Cancel
          </Button>
        </>
      );
    }
    return null;
  }

  if (mode === "history") {
    return (
      <YouTubeHistoryPage
        entries={historyEntries}
        loading={historyLoading}
        error={historyError}
        onOpenFolder={async (entry) => {
          try {
            await openYouTubeDownloadFolder({
              runId: entry.runId,
              occurrenceId: null,
              outputDir: entry.outputDir
            });
          } catch (error) {
            toast.error("Could not open folder", { description: formatYouTubeInvokeError(error) });
          }
        }}
      />
    );
  }

  return (
    <div className="youtube-view" data-has-results={videos.length > 0 || isScanning || undefined}>
      <div className="youtube-live-announcer" role="status" aria-live="polite">{liveAnnouncement}</div>
      {helperStatus === "failed" ? (
        <div className="youtube-helper-error" role="alert">
          {helperError?.message ?? "YouTube helper integrity validation failed; native discovery remains blocked."}
        </div>
      ) : null}

      <div className="youtube-search-stage">
        <div className="youtube-search-field">
          <Textarea
            ref={sourceInputRef}
            value={sourceUrl}
            onChange={(event) => handleSourceChange(event.target.value)}
            onPaste={handlePaste}
            onPointerDown={handleSourcePointerDown}
            onFocus={handleSourceFocus}
            onKeyDown={handleSearchKeyDown}
            placeholder="Paste a YouTube URL, or several links at once"
            aria-label="Public YouTube URL"
            spellCheck={false}
            rows={1}
            disabled={activeRun}
            className="youtube-search-input min-h-0"
          />
        </div>

        <div className="youtube-control-cluster youtube-options-row">
          <label className="youtube-cluster-field youtube-option-mode">
            <span>Mode</span>
            <Select value={downloadMode} onChange={(event) => setDownloadMode(event.target.value as YouTubeDownloadMode)} disabled={activeRun} aria-label="YouTube capture mode">
              <option value="video_and_transcript">Video + transcript</option>
              <option value="video_only">Video only</option>
              <option value="transcript_only">Transcript only</option>
            </Select>
          </label>
          <label className="youtube-cluster-field youtube-option-quality">
            <span>Quality</span>
            <Select value={String(maxHeight ?? "best")} onChange={(event) => setMaxHeight(event.target.value === "best" ? null : Number(event.target.value) as StartYouTubeDownloadRequest["maxHeight"])} disabled={activeRun} aria-label="Maximum video height">
              <option value="best">Best available</option>
              <option value="2160">2160p</option>
              <option value="1440">1440p</option>
              <option value="1080">1080p</option>
              <option value="720">720p</option>
              <option value="480">480p</option>
            </Select>
          </label>
          <label className="youtube-cluster-field youtube-option-language">
            <span>{isDetectingLanguages ? "Language…" : "Language"}</span>
            <Select
              value={preferredLanguage ?? ""}
              onChange={(event) => setPreferredLanguage(event.target.value.trim() || null)}
              onFocus={() => ensureLanguageOptionsForSelection()}
              disabled={activeRun || isDetectingLanguages}
              aria-label="Preferred transcript language"
            >
              {languageOptions.map((option) => (
                <option key={option.tag || "no-caption"} value={option.tag}>{option.label}</option>
              ))}
            </Select>
          </label>
          <label className="youtube-cluster-field youtube-cluster-folder">
            <span>Save to</span>
            <button
              type="button"
              className="youtube-folder-field"
              onClick={() => void pickOutputDirectory()}
              disabled={activeRun}
              aria-label="YouTube output directory"
              title={outputDir || "Choose a folder"}
            >
              <Folder aria-hidden="true" />
              <span className="youtube-folder-path">{outputDir || "Choose a folder"}</span>
            </button>
          </label>
          {ambiguousPlaylistSource ? (
            <label className="youtube-cluster-field youtube-option-playlist">
              <span>Video + playlist</span>
              <Select
                value={playlistMode}
                onChange={(event) => handlePlaylistModeChange(event.target.value as YouTubePlaylistMode)}
                disabled={isScanning || activeRun}
                aria-label="When URL includes a video and playlist"
              >
                <option value="video">This video only</option>
                <option value="playlist">Entire playlist</option>
              </Select>
            </label>
          ) : null}
        </div>
      </div>

      <section className="youtube-results" aria-label="Detected YouTube links">
          {multipleResults || activeRun ? (
            <div className="youtube-results-toolbar">
              {multipleResults && showQueueDownloads ? (
                <Button
                  type="button"
                  size="xs"
                  variant="primary"
                  className="youtube-download-overlay-button"
                  onClick={() => void handleStart()}
                  loading={isStarting}
                  loadingLabel="Starting"
                  disabled={availableVideos.length === 0 || !helperReady || activeRun}
                >
                  Download all
                </Button>
              ) : null}
              {renderRunControls()}
            </div>
          ) : null}
          <div className="queue-section-tabs youtube-queue-section-tabs" role="group" aria-label="YouTube download queue sections">
            <YouTubeQueueSectionTab
              section="queue"
              label="Queue"
              value={queueCounts.queue}
              tone="queue"
              selected={queueSection === "queue"}
              onClick={() => setQueueSection("queue")}
            />
            <YouTubeQueueSectionTab
              section="active"
              label="Active"
              value={queueCounts.active}
              tone="primary"
              selected={queueSection === "active"}
              onClick={() => setQueueSection("active")}
            />
            <YouTubeQueueSectionTab
              section="completed"
              label="Completed"
              value={queueCounts.completed}
              tone="success"
              selected={queueSection === "completed"}
              onClick={() => setQueueSection("completed")}
            />
            <YouTubeQueueSectionTab
              section="failed"
              label="Failed"
              value={queueCounts.failed}
              tone="danger"
              selected={queueSection === "failed"}
              onClick={() => setQueueSection("failed")}
            />
          </div>
          <div
            className={`queue-section-panel queue-section-panel-${queueSection}`}
            aria-label={`${queueSection} YouTube downloads`}
          >
            <ol className="youtube-result-list" aria-label="Scanned YouTube occurrences">
              {sectionVideos.map(({ video, state }) => {
                const outcome = runItems.get(video.item.occurrenceId);
                const available = video.item.availability === "available";
                const availabilityLabel = video.item.availability === "unknown" ? "Unconfirmed" : "Unavailable";
                const status = itemStatusText(state, available);
                const warningText = outcome?.warnings?.length
                  ? formatYouTubeWarning(outcome.warnings[0] ?? "")
                  : null;
                const percent = itemProgressPercent(video.item.occurrenceId, state, runSnapshot, currentProgress);
                const duration = formatDuration(video.item.durationSeconds);
                const plan = scanPlans.find((candidate) => candidate.scanPlanId === video.scanPlanId);
                const kind = plan?.kind === "playlist" ? "playlist" : "video";
                const meta = [detectedKindLabel(kind), video.item.channelName, duration].filter(Boolean).join(" · ");
                const showRowDownload = showQueueDownloads && !multipleResults;
                const showInlineDownload = showQueueDownloads && multipleResults && available;
                const showFileAction = queueSection === "completed"
                  && (state === "completed" || state === "completed_with_warnings" || state === "skipped_existing");
                return (
                  <li key={video.item.occurrenceId} className="youtube-result-row" data-state={state} data-unavailable={!available || undefined}>
                    <div className="youtube-result-copy">
                      <strong>{video.item.title}</strong>
                      <span>{meta || video.item.sourceUrl}</span>
                      {percent !== null ? (
                        <div className="youtube-result-progress">
                          <Progress value={percent} />
                          <span>{status ?? "Downloading"}{percent > 0 ? ` · ${Math.round(percent)}%` : ""}</span>
                        </div>
                      ) : status ? <span className="youtube-result-status">{status}</span> : null}
                      {warningText ? <span className="youtube-result-warning">{warningText}</span> : null}
                      {!available ? <span className="youtube-result-status">{availabilityLabel}</span> : null}
                    </div>
                    <div className="youtube-result-overlay">
                      {showFileAction ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          className="youtube-file-action"
                          onClick={() => void openCompletedOccurrenceFolder(video.item.occurrenceId)}
                          aria-label={`Open download folder for ${video.item.title}`}
                        >
                          <FolderOpen aria-hidden="true" />
                          File
                        </Button>
                      ) : null}
                      {showRowDownload ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="primary"
                          className="youtube-download-overlay-button"
                          onClick={() => void handleDownloadOne(video)}
                          loading={isStarting}
                          loadingLabel="Starting"
                          disabled={!available || activeRun || !helperReady}
                          aria-label={`Download occurrence ${video.item.ordinal}: ${video.item.title}`}
                        >
                          Download
                        </Button>
                      ) : null}
                      {showInlineDownload ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          onClick={() => void handleDownloadOne(video)}
                          disabled={!available || activeRun || !helperReady}
                          aria-label={`Download occurrence ${video.item.ordinal}: ${video.item.title}`}
                        >
                          Download
                        </Button>
                      ) : null}
                      {!multipleResults && queueSection === "active" ? renderRunControls() : null}
                    </div>
                  </li>
                );
              })}
              {queueSection === "queue" && isScanning && videos.length === 0 ? (
                <YouTubeScanSkeletonRows count={YOUTUBE_SCAN_SKELETON_COUNT} />
              ) : null}
              {queueSection === "queue" && isScanning && videos.length > 0 ? (
                <YouTubeScanSkeletonRows count={1} />
              ) : null}
              {!isScanning && sectionVideos.length === 0 ? (
                <li className="youtube-result-empty" aria-live="polite">
                  {queueSection === "queue" && videos.length === 0
                    ? "Paste a URL to scan"
                    : `No ${queueSection} videos`}
                </li>
              ) : null}
            </ol>
            {isScanning && queueSection === "queue" ? (
              <p className="youtube-scanning" role="status">Finding videos…</p>
            ) : null}
          </div>
        </section>

      <Dialog
        open={folderGateOpen}
        onOpenChange={setFolderGateOpen}
        title="Choose a destination folder"
        description="Pick where YouTube downloads should be saved before pasting a link."
        className="youtube-folder-gate-dialog"
      >
        <div className="youtube-folder-gate-actions">
          <Button type="button" variant="outline" onClick={() => setFolderGateOpen(false)}>
            Cancel
          </Button>
          <Button type="button" variant="primary" onClick={() => void confirmDestinationFolderFromGate()}>
            <Folder aria-hidden="true" />
            Choose destination folder
          </Button>
        </div>
      </Dialog>
    </div>
  );
}

function formatYouTubeHistoryDate(timestamp: number): string {
  if (!timestamp) return "—";
  return new Date(timestamp * 1000).toLocaleString([], {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  });
}

function formatYouTubeHistoryState(state: string): string {
  return state.split("_").join(" ");
}

function YouTubeHistoryPage({
  entries,
  loading,
  error,
  onOpenFolder
}: {
  entries: YouTubeHistoryEntry[];
  loading: boolean;
  error: string | null;
  onOpenFolder: (entry: YouTubeHistoryEntry) => void | Promise<void>;
}) {
  return (
    <div className="lv-workspace download-history-workspace youtube-history-workspace">
      <div className="download-history-header youtube-history-header">
        <p className="download-history-count">
          {loading
            ? "Loading history…"
            : `${entries.length} completed download${entries.length === 1 ? "" : "s"}`}
        </p>
      </div>
      {error ? (
        <div className="youtube-helper-error" role="alert">{error}</div>
      ) : null}
      {!loading && entries.length === 0 && !error ? (
        <div className="download-history-empty youtube-history-empty" role="status">
          No downloaded YouTube history yet.
        </div>
      ) : null}
      {entries.length > 0 ? (
        <ol className="download-history-list youtube-history-list" aria-label="YouTube download history">
          {entries.map((entry) => {
            const when = formatYouTubeHistoryDate(entry.completedAt ?? entry.createdAt);
            const meta = [
              formatYouTubeHistoryState(entry.state),
              when,
              `${entry.videoCount} video${entry.videoCount === 1 ? "" : "s"}`
            ].join(" · ");
            return (
              <li key={entry.runId} className="download-history-row youtube-history-row">
                <div className="download-history-copy">
                  <strong>{entry.title}</strong>
                  <span>{meta}</span>
                  {entry.errorMessage ? (
                    <span className="youtube-result-warning">{entry.errorMessage}</span>
                  ) : null}
                </div>
                <div className="download-history-overlay">
                  <Button
                    type="button"
                    size="xs"
                    variant="ghost"
                    className="download-history-file-action youtube-file-action"
                    onClick={() => void onOpenFolder(entry)}
                    aria-label={`Open folder for ${entry.title}`}
                  >
                    <FolderOpen aria-hidden="true" />
                    Open Folder
                  </Button>
                </div>
              </li>
            );
          })}
        </ol>
      ) : null}
    </div>
  );
}
