import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type FocusEvent, type KeyboardEvent, type PointerEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  Folder,
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
  pauseYouTubeDownload,
  readActiveYouTubePreviewScan,
  resumeYouTubeDownload,
  formatYouTubeInvokeError,
  scanYouTubeSource,
  startYouTubeDownload,
  subscribeYouTubeRunChanged,
  youtubeErrorFromUnknown,
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
  type YouTubeItemState,
  type YouTubePlaylistMode,
  type YouTubeRunSnapshot,
  type YouTubeScanItem,
  type YouTubeTranscriptTrack
} from "../../lib/youtube/types";
import { loadYouTubeOutputDir, persistYouTubeOutputDir, readPreviewYouTubeOutputDir } from "../../lib/youtube/preferences";

type HelperStatus = "pending" | "ready" | "failed";

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

function formatYouTubeWarning(code: string): string {
  switch (code) {
    case "TRANSCRIPT_MISSING":
      return "No captions were available on YouTube for this video, so only the media file was saved.";
    default:
      return code.trim() || "Completed with warnings.";
  }
}

export function YouTubeView() {
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
  const [mode, setMode] = useState<YouTubeDownloadMode>("video_and_transcript");
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
  const sourceInputRef = useRef<HTMLTextAreaElement | null>(null);
  const transcriptInspectionGenerationRef = useRef(0);
  const latestRunIdRef = useRef<string | null>(null);
  const latestRevisionRef = useRef(0);
  const scanGenerationRef = useRef(0);
  const playlistModeRef = useRef(playlistMode);
  playlistModeRef.current = playlistMode;
  const lastFingerprintRef = useRef("");
  const downloadQueueRef = useRef<DownloadGroup[]>([]);
  const startingGroupRef = useRef(false);
  const warnedRunIdRef = useRef<string | null>(null);
  const hasDestinationFolder = outputDir.trim().length > 0;
  const ambiguousPlaylistSource = detectedLinks.some((link) => link.kind === "ambiguous")
    || scanPlans.some((plan) => isAmbiguousWatchPlaylist(plan.canonicalUrl));

  const syncSearchInputHeight = useCallback(() => {
    const el = sourceInputRef.current;
    if (!el) return;
    el.style.height = "0px";
    const next = Math.min(
      Math.max(el.scrollHeight, YOUTUBE_SEARCH_MIN_HEIGHT_PX),
      YOUTUBE_SEARCH_MAX_HEIGHT_PX
    );
    el.style.height = `${next}px`;
  }, []);

  useLayoutEffect(() => {
    syncSearchInputHeight();
  }, [sourceUrl, syncSearchInputHeight]);

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
  }, [runSnapshot?.runId]);

  const availableVideos = useMemo(
    () => videos.filter((video) => video.item.availability === "available"),
    [videos]
  );
  const runItems = useMemo(
    () => new Map((runSnapshot?.items ?? []).map((item) => [item.occurrenceId, item])),
    [runSnapshot]
  );
  const activeRun = runSnapshot !== null && !isYouTubeRunTerminal(runSnapshot.state);
  const canPauseRun = activeRun && runSnapshot?.state === "running";
  const canResumeRun = activeRun && (runSnapshot?.state === "paused" || runSnapshot?.state === "pause_requested");
  const currentProgress = runSnapshot?.progress.fraction === null || runSnapshot?.progress.fraction === undefined
    ? 0
    : Math.max(0, Math.min(1, runSnapshot.progress.fraction)) * 100;
  const multipleResults = videos.length > 1;
  const fallbackLanguages = useMemo(
    () => languageOptions
      .map((option) => option.tag)
      .filter((tag) => tag !== preferredLanguage)
      .slice(0, 8),
    [languageOptions, preferredLanguage]
  );
  const liveAnnouncement = runSnapshot
    ? `${runSnapshot.state}. ${runSnapshot.counts.completed} of ${runSnapshot.counts.selected} complete${runSnapshot.item ? `. ${runSnapshot.item.title}` : ""}.`
    : videos.length > 0
      ? `${videos.length} video${videos.length === 1 ? "" : "s"} detected.`
      : "Paste a YouTube link to detect videos.";

  useEffect(() => {
    if (!runSnapshot || runSnapshot.state !== "completed_with_warnings") return;
    if (warnedRunIdRef.current === runSnapshot.runId) return;
    warnedRunIdRef.current = runSnapshot.runId;
    const codes = [
      ...runSnapshot.warnings.map((warning) => warning.code),
      ...runSnapshot.items.flatMap((item) => item.warnings)
    ];
    const first = codes.find((code) => code === "TRANSCRIPT_MISSING") ?? codes[0] ?? null;
    if (!first) return;
    toast.warning("Saved with warnings", { description: formatYouTubeWarning(first) });
  }, [runSnapshot]);

  function resetDetectedResults(): void {
    transcriptInspectionGenerationRef.current += 1;
    scanGenerationRef.current += 1;
    lastFingerprintRef.current = "";
    setScanPlans([]);
    setVideos([]);
    setSelectedOccurrenceIds(new Set());
    setLanguageOptions([NO_CAPTION_OPTION]);
    setPreferredLanguage(null);
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

  async function refreshDetectedLanguages(nextVideos: ResultVideo[], generation: number): Promise<void> {
    const groups = groupsForVideos(nextVideos);
    if (groups.length === 0) {
      setLanguageOptions([NO_CAPTION_OPTION]);
      setPreferredLanguage(null);
      return;
    }
    setIsDetectingLanguages(true);
    const tracks: YouTubeTranscriptTrack[] = [];
    try {
      for (const group of groups) {
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
      if (generation === scanGenerationRef.current) setIsDetectingLanguages(false);
    }
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
    setScanPlans([]);
    setVideos([]);
    setSelectedOccurrenceIds(new Set());
    setLanguageOptions([NO_CAPTION_OPTION]);
    setPreferredLanguage(null);
    setIsScanning(true);
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
        await refreshDetectedLanguages(mergedVideos, generation);
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
    const links = detectYouTubeLinks(next);
    setSourceUrl(next);
    setDetectedLinks(links);
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
    if (!next.trim()) return;
    requestAutoScan(links);
  }

  function handleSearchKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (!ensureDestinationFolder()) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
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
      if (mode !== "video_only") {
        const inspected = await requestTranscriptInspection(group.scanPlanId, group.occurrenceIds);
        if (!inspected) return false;
        const missingCaptions = inspected.occurrences.some((occurrence) => occurrence.tracks.length === 0);
        if (missingCaptions && mode === "video_and_transcript") {
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
        mode,
        maxHeight,
        preferredLanguage,
        fallbackLanguages,
        allowAutomaticCaptions: true,
        continueWithoutTranscript: true
      });
      latestRunIdRef.current = response.receipt.runId;
      latestRevisionRef.current = 0;
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
            className="youtube-search-input"
          />
        </div>

        <div className="youtube-control-cluster youtube-options-row">
          <label className="youtube-cluster-field youtube-option-mode">
            <span>Mode</span>
            <Select value={mode} onChange={(event) => setMode(event.target.value as YouTubeDownloadMode)} disabled={activeRun} aria-label="YouTube capture mode">
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

      {videos.length > 0 || isScanning ? (
        <section className="youtube-results" aria-label="Detected YouTube links">
          {multipleResults || activeRun ? (
            <div className="youtube-results-toolbar">
              {multipleResults && !activeRun ? (
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
          <ol className="youtube-result-list" aria-label="Scanned YouTube occurrences">
            {videos.map((video) => {
              const outcome = runItems.get(video.item.occurrenceId);
              const available = video.item.availability === "available";
              const state: YouTubeItemState = outcome?.state ?? (available ? "pending" : "skipped");
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
              const showRowDownload = !multipleResults && !activeRun;
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
                    {multipleResults && !activeRun && available ? (
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
                    {!multipleResults ? renderRunControls() : null}
                  </div>
                </li>
              );
            })}
          </ol>
          {isScanning && videos.length === 0 ? (
            <p className="youtube-scanning" role="status">Finding videos…</p>
          ) : null}
        </section>
      ) : null}

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
