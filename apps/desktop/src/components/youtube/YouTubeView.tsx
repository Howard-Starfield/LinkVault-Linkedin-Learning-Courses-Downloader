import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  CircleHelp,
  Folder,
  ListVideo,
  Play,
  RefreshCw,
  ScanLine,
  Square,
  X
} from "lucide-react";
import {
  Button,
  EmptyRow,
  Field,
  Input,
  Panel,
  Progress,
  Select,
  StatusBadge
} from "../primitives";
import {
  cancelYouTubeDownload,
  getYouTubeDownloadState,
  getYouTubeHelperStatus,
  isTauriRuntime,
  scanYouTubeSource,
  startYouTubeDownload,
  subscribeYouTubeRunChanged
} from "../../lib/youtube/ipc";
import {
  isYouTubeRunTerminal,
  type ScanYouTubeSourceResponse,
  type StartYouTubeDownloadRequest,
  type YouTubeDownloadMode,
  type YouTubeError,
  type YouTubeItemState,
  type YouTubeRunSnapshot,
  type YouTubeRunState,
  type YouTubeScanItem
} from "../../lib/youtube/types";

const ACKNOWLEDGEMENT_KEY = "linkvault.youtube.internal-acknowledgement.v1";
const FIRST_USE_ACKNOWLEDGEMENT =
  "YouTube Downloader is for public videos and playlists you own or are authorized to save. Do not use LinkVault with private, member-only, paid, age-gated or otherwise restricted content. LinkVault does not grant permission, bypass access controls, or provide legal advice.";
const PERSISTENT_GUIDANCE =
  "Use this internal feature only with public content you own or are authorized to save. Cookies, accounts, restricted-content access, DRM/access-control bypass and public distribution are not supported.";

type HelperStatus = "pending" | "ready" | "failed";

function helperStatusFailure(error: unknown): YouTubeError {
  return {
    code: "HELPER_STATUS_UNAVAILABLE",
    message: error instanceof Error ? error.message : String(error)
  };
}

function readAcknowledgement(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(ACKNOWLEDGEMENT_KEY) === "true";
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
  if (seconds === null || !Number.isFinite(seconds)) return "Duration unknown";
  const total = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(total / 60);
  const remainder = total % 60;
  return `${minutes}:${String(remainder).padStart(2, "0")}`;
}

function formatOrdinal(ordinal: number): string {
  return String(Math.max(0, ordinal)).padStart(2, "0");
}

function formatBytes(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes)) return "—";
  if (bytes < 1_000) return `${Math.round(bytes)} B`;
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(1)} kB`;
  if (bytes < 1_000_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}

function runTone(state: YouTubeRunState | null): "neutral" | "primary" | "success" | "danger" | "muted" {
  if (state === "completed") return "success";
  if (state === "completed_with_warnings") return "primary";
  if (state === "failed" || state === "cancelled") return "danger";
  if (state === "running" || state === "pause_requested" || state === "cancelling") return "primary";
  return "muted";
}

function runLabel(state: YouTubeRunState | null): string {
  switch (state) {
    case "running": return "Running";
    case "pause_requested": return "Finishing current item";
    case "paused": return "Paused";
    case "cancelling": return "Cancelling";
    case "completed": return "Completed";
    case "completed_with_warnings": return "Completed with warnings";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    default: return "No run";
  }
}

function itemStateLabel(state: YouTubeItemState): string {
  switch (state) {
    case "completed": return "Completed";
    case "completed_with_warnings": return "Warning";
    case "running": return "In progress";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    case "skipped": return "Skipped";
    case "skipped_existing": return "Already saved";
    default: return "Queued";
  }
}

function itemStateTone(state: YouTubeItemState): "neutral" | "primary" | "success" | "danger" | "muted" {
  if (state === "completed" || state === "skipped_existing") return "success";
  if (state === "running" || state === "completed_with_warnings") return "primary";
  if (state === "failed" || state === "cancelled") return "danger";
  return "muted";
}

export function YouTubeView() {
  const nativeRuntime = isTauriRuntime();
  const [helperStatus, setHelperStatus] = useState<HelperStatus>(nativeRuntime ? "pending" : "ready");
  const [helperError, setHelperError] = useState<YouTubeError | null>(null);
  const helperReady = !nativeRuntime || helperStatus === "ready";
  const [acknowledged, setAcknowledged] = useState(readAcknowledgement);
  const [sourceUrl, setSourceUrl] = useState("");
  const [scan, setScan] = useState<ScanYouTubeSourceResponse | null>(null);
  const [selectedOccurrenceIds, setSelectedOccurrenceIds] = useState<Set<string>>(() => new Set());
  const [outputDir, setOutputDir] = useState("");
  const [mode, setMode] = useState<YouTubeDownloadMode>("video_and_transcript");
  const [maxHeight, setMaxHeight] = useState<StartYouTubeDownloadRequest["maxHeight"]>(1080);
  const [allowAutomaticCaptions, setAllowAutomaticCaptions] = useState(true);
  const [continueWithoutTranscript, setContinueWithoutTranscript] = useState(true);
  const [isScanning, setIsScanning] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [runSnapshot, setRunSnapshot] = useState<YouTubeRunSnapshot | null>(null);
  const latestRunIdRef = useRef<string | null>(null);
  const latestRevisionRef = useRef(0);

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

  const applyRunSnapshot = useCallback((snapshot: YouTubeRunSnapshot | null) => {
    if (!snapshot) return;
    const sameRun = latestRunIdRef.current === snapshot.runId;
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
          void getYouTubeDownloadState({ runId: event.runId })
            .then((snapshot) => {
              if (!disposed) applyRunSnapshot(snapshot);
            })
            .catch(() => undefined);
        });
        if (disposed) cleanup();
        else unlisten = cleanup;
        const snapshot = await getYouTubeDownloadState({ runId: null });
        if (!disposed) applyRunSnapshot(snapshot);
      } catch (error) {
        if (!disposed && nativeRuntime) {
          toast.error("YouTube runtime state unavailable", { description: String(error) });
        }
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyRunSnapshot, nativeRuntime]);

  const selectedCount = selectedOccurrenceIds.size;
  const selectedItems = useMemo(
    () => scan?.items.filter((item) => selectedOccurrenceIds.has(item.occurrenceId)) ?? [],
    [scan, selectedOccurrenceIds]
  );
  const availableCount = useMemo(
    () => scan?.items.filter((item) => item.availability !== "unavailable").length ?? 0,
    [scan]
  );
  const runItems = useMemo(
    () => new Map((runSnapshot?.items ?? []).map((item) => [item.occurrenceId, item])),
    [runSnapshot]
  );
  const activeRun = runSnapshot !== null && !isYouTubeRunTerminal(runSnapshot.state);
  const currentProgress = runSnapshot?.progress.fraction === null || runSnapshot?.progress.fraction === undefined
    ? 0
    : Math.max(0, Math.min(1, runSnapshot.progress.fraction)) * 100;
  const liveAnnouncement = runSnapshot
    ? `${runLabel(runSnapshot.state)}. ${runSnapshot.counts.completed} of ${runSnapshot.counts.selected} occurrences complete${runSnapshot.item ? `. Current item: ${runSnapshot.item.title}` : ""}.`
    : scan
      ? `${scan.items.length} ${scan.kind} occurrence${scan.items.length === 1 ? "" : "s"} ready for selection.`
      : "YouTube source reel is empty.";

  function setAcknowledgement(next: boolean): void {
    setAcknowledged(next);
    if (typeof window !== "undefined") {
      if (next) window.localStorage.setItem(ACKNOWLEDGEMENT_KEY, "true");
      else window.localStorage.removeItem(ACKNOWLEDGEMENT_KEY);
    }
  }

  function toggleOccurrence(item: YouTubeScanItem): void {
    if (item.availability === "unavailable") return;
    setSelectedOccurrenceIds((current) => {
      const next = new Set(current);
      if (next.has(item.occurrenceId)) next.delete(item.occurrenceId);
      else next.add(item.occurrenceId);
      return next;
    });
  }

  function selectAllOccurrences(): void {
    if (!scan) return;
    const available = scan.items.filter((item) => item.availability !== "unavailable");
    const allSelected = available.length > 0 && available.every((item) => selectedOccurrenceIds.has(item.occurrenceId));
    setSelectedOccurrenceIds(allSelected ? new Set() : new Set(available.map((item) => item.occurrenceId)));
  }

  async function handleScan(): Promise<void> {
    if (!helperReady) {
      toast.error("YouTube helper is not ready", { description: "The exact Y0 helper lock and integrity gate must pass before native discovery." });
      return;
    }
    if (!acknowledged) {
      toast.error("Confirm the internal-use acknowledgement first");
      return;
    }
    if (!sourceUrl.trim()) {
      toast.error("Enter a YouTube URL first");
      return;
    }
    setIsScanning(true);
    try {
      const nextScan = await scanYouTubeSource({
        clientOperationId: createClientId("youtube-operation"),
        url: sourceUrl.trim()
      });
      setScan(nextScan);
      setSelectedOccurrenceIds(new Set(nextScan.items.filter((item) => item.availability !== "unavailable").map((item) => item.occurrenceId)));
      toast.success("Source scanned", { description: `${nextScan.items.length} occurrence${nextScan.items.length === 1 ? "" : "s"} in source order.` });
    } catch (error) {
      toast.error("YouTube scan failed", { description: String(error) });
    } finally {
      setIsScanning(false);
    }
  }

  async function browseOutputDirectory(): Promise<void> {
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: outputDir || undefined });
      if (typeof picked === "string" && picked.trim()) setOutputDir(picked);
    } catch (error) {
      toast.error("Folder picker failed", { description: String(error) });
    }
  }

  async function handleStart(): Promise<void> {
    if (!scan || selectedItems.length === 0 || !outputDir.trim()) {
      toast.error("Choose at least one occurrence and an output directory");
      return;
    }
    if (!helperReady || !acknowledged) return;
    setIsStarting(true);
    try {
      const response = await startYouTubeDownload({
        clientSubmissionId: createClientId("youtube-submission"),
        scanPlanId: scan.scanPlanId,
        selectedOccurrenceIds: selectedItems.map((item) => item.occurrenceId),
        outputDir: outputDir.trim(),
        mode,
        maxHeight,
        preferredLanguage: null,
        fallbackLanguages: [],
        allowAutomaticCaptions,
        continueWithoutTranscript
      });
      const snapshot = await getYouTubeDownloadState({ runId: response.runId });
      applyRunSnapshot(snapshot);
    } catch (error) {
      toast.error("YouTube download could not start", { description: String(error) });
    } finally {
      setIsStarting(false);
    }
  }

  async function handleCancel(): Promise<void> {
    if (!runSnapshot || !activeRun) return;
    setIsCancelling(true);
    try {
      const snapshot = await cancelYouTubeDownload({ runId: runSnapshot.runId });
      applyRunSnapshot(snapshot);
    } catch (error) {
      toast.error("Could not cancel YouTube run", { description: String(error) });
    } finally {
      setIsCancelling(false);
    }
  }

  function clearScan(): void {
    setScan(null);
    setSelectedOccurrenceIds(new Set());
  }

  const helperMessage = nativeRuntime
    ? helperStatus === "ready"
      ? "Y0 helper lock validated; native discovery is enabled."
      : helperStatus === "failed"
        ? helperError?.message ?? "Y0 helper integrity failed; native discovery and downloads are blocked."
        : "Y0 helper lock validation is pending; native discovery and downloads are blocked."
    : "Browser preview uses deterministic fixture data; no helper is launched.";

  return (
    <div className="youtube-view">
      <header className="youtube-view-heading">
        <div className="youtube-view-mark" aria-hidden="true">YT</div>
        <div className="min-w-0">
          <h2>YouTube archive</h2>
          <p>Scan a source, review its ordered occurrences, then save an authorized internal copy.</p>
        </div>
        <StatusBadge tone={helperReady ? "success" : "muted"}>
          {helperReady ? "Helper gate ready" : "Helper gate blocked"}
        </StatusBadge>
      </header>

      <div className="youtube-guidance" role="note">
        <div className="youtube-guidance-icon"><CircleHelp aria-hidden="true" /></div>
        <div className="youtube-guidance-copy">
          <strong>Internal-use guardrail</strong>
          <p>{PERSISTENT_GUIDANCE}</p>
          <label className="youtube-acknowledgement">
            <input
              type="checkbox"
              checked={acknowledged}
              onChange={(event) => setAcknowledgement(event.target.checked)}
            />
            <span>{FIRST_USE_ACKNOWLEDGEMENT}</span>
          </label>
        </div>
      </div>

      <div className={`youtube-helper-status youtube-helper-status-${helperStatus}`} role="status">
        {helperReady ? <CheckCircle2 aria-hidden="true" /> : <AlertTriangle aria-hidden="true" />}
        <span>{helperMessage}</span>
      </div>
      {helperStatus === "failed" ? (
        <div className="youtube-helper-error" role="alert">
          {helperError?.message ?? "YouTube helper integrity validation failed; native discovery remains blocked."}
        </div>
      ) : null}

      <Panel className="youtube-command-panel">
        <div className="youtube-command-heading">
          <div>
            <span className="youtube-kicker">01 / SOURCE</span>
            <h3>Build a source reel</h3>
          </div>
          <StatusBadge tone={acknowledged ? "success" : "muted"}>{acknowledged ? "Acknowledged" : "Acknowledgement required"}</StatusBadge>
        </div>
        <div className="youtube-command-grid">
          <Field label="Public YouTube URL">
            <div className="youtube-url-row">
              <Input
                value={sourceUrl}
                onChange={(event) => setSourceUrl(event.target.value)}
                onKeyDown={(event) => {
                  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") void handleScan();
                }}
                placeholder="https://www.youtube.com/watch?v=..."
                aria-label="Public YouTube URL"
                spellCheck={false}
                disabled={!helperReady || !acknowledged || isScanning || activeRun}
              />
              <Button type="button" variant="primary" onClick={() => void handleScan()} loading={isScanning} loadingLabel="Scanning" disabled={!helperReady || !acknowledged || activeRun}>
                <ScanLine aria-hidden="true" />
                Scan
              </Button>
            </div>
          </Field>
          <Field label="Output directory">
            <div className="youtube-output-row">
              <Input value={outputDir} onChange={(event) => setOutputDir(event.target.value)} aria-label="YouTube output directory" placeholder="Choose a local destination" disabled={activeRun} />
              <Button type="button" variant="outline" onClick={() => void browseOutputDirectory()} disabled={activeRun}>
                <Folder aria-hidden="true" />
                Browse
              </Button>
            </div>
          </Field>
        </div>
        <div className="youtube-options-row">
          <Field label="Capture mode">
            <Select value={mode} onChange={(event) => setMode(event.target.value as YouTubeDownloadMode)} disabled={activeRun} aria-label="YouTube capture mode">
              <option value="video_and_transcript">Video + transcript</option>
              <option value="video_only">Video only</option>
              <option value="transcript_only">Transcript only</option>
            </Select>
          </Field>
          <Field label="Maximum height">
            <Select value={String(maxHeight ?? "best")} onChange={(event) => setMaxHeight(event.target.value === "best" ? null : Number(event.target.value) as StartYouTubeDownloadRequest["maxHeight"])} disabled={activeRun} aria-label="Maximum video height">
              <option value="best">Best available</option>
              <option value="2160">2160p</option>
              <option value="1440">1440p</option>
              <option value="1080">1080p</option>
              <option value="720">720p</option>
              <option value="480">480p</option>
            </Select>
          </Field>
          <label className="youtube-option-check"><input type="checkbox" checked={allowAutomaticCaptions} onChange={(event) => setAllowAutomaticCaptions(event.target.checked)} disabled={activeRun} /><span>Allow automatic captions</span></label>
          <label className="youtube-option-check"><input type="checkbox" checked={continueWithoutTranscript} onChange={(event) => setContinueWithoutTranscript(event.target.checked)} disabled={activeRun} /><span>Continue if transcript is missing</span></label>
        </div>
      </Panel>

      <div className="youtube-stage-grid">
        <Panel className="youtube-reel-panel">
          <div className="youtube-panel-heading">
            <div>
              <span className="youtube-kicker">02 / REVIEW</span>
              <h3>Source reel</h3>
            </div>
            <div className="youtube-panel-actions">
              {scan ? <span className="youtube-count-label">{selectedCount} / {scan.items.length} selected</span> : null}
              <Button type="button" size="xs" variant="ghost" onClick={selectAllOccurrences} disabled={!scan || activeRun}>{scan && availableCount > 0 && selectedCount === availableCount ? "Clear" : "Select all"}</Button>
              <Button type="button" size="xs" variant="ghost" onClick={clearScan} disabled={!scan || activeRun} aria-label="Clear scanned source"><X aria-hidden="true" /></Button>
            </div>
          </div>
          {scan ? (
            <>
              <div className="youtube-scan-summary">
                <ListVideo aria-hidden="true" />
                <div className="min-w-0"><strong>{scan.title}</strong><span>{scan.kind === "playlist" ? `${scan.items.length} playlist occurrences` : "Single video occurrence"}{scan.truncated ? " · truncated" : ""}</span></div>
                <span className="youtube-scan-id">{scan.sourceId}</span>
              </div>
              <ol className="youtube-source-reel" aria-label="Scanned YouTube occurrences">
                {scan.items.map((item) => {
                  const outcome = runItems.get(item.occurrenceId);
                  const state: YouTubeItemState = outcome?.state ?? (item.availability === "unavailable" ? "skipped" : "pending");
                  const unavailable = item.availability === "unavailable";
                  return (
                    <li key={item.occurrenceId} className="youtube-reel-item" data-state={state} data-unavailable={unavailable || undefined}>
                      <label className="youtube-reel-select">
                        <input type="checkbox" checked={selectedOccurrenceIds.has(item.occurrenceId)} onChange={() => toggleOccurrence(item)} disabled={unavailable || activeRun} aria-label={`Select occurrence ${item.ordinal}: ${item.title}`} />
                        <span className="youtube-reel-check" aria-hidden="true"><Check /></span>
                        <span className="youtube-reel-ordinal" aria-hidden="true">{formatOrdinal(item.ordinal)}</span>
                        <span className="youtube-reel-copy"><strong>{item.title}</strong><span>{item.channelName ?? "Channel unavailable"} · {formatDuration(item.durationSeconds)}</span></span>
                        <StatusBadge tone={itemStateTone(state)}>{unavailable ? "Unavailable" : itemStateLabel(state)}</StatusBadge>
                      </label>
                    </li>
                  );
                })}
              </ol>
            </>
          ) : <EmptyRow title="No source scanned" description="Paste a public YouTube URL above, then scan to build the ordered reel." />}
        </Panel>

        <Panel className="youtube-run-panel">
          <div className="youtube-panel-heading">
            <div>
              <span className="youtube-kicker">03 / RUN LEDGER</span>
              <h3>Archive progress</h3>
            </div>
            <StatusBadge tone={runTone(runSnapshot?.state ?? null)}>{runLabel(runSnapshot?.state ?? null)}</StatusBadge>
          </div>
          <div className="youtube-live-announcer" role="status" aria-live="polite">{liveAnnouncement}</div>
          {runSnapshot ? (
            <div className="youtube-progress-block">
              <div className="youtube-progress-meta"><span>{runSnapshot.item ? runSnapshot.item.title : "Run settled"}</span><span>{Math.round(currentProgress)}%</span></div>
              <Progress value={currentProgress} />
              <div className="youtube-progress-detail"><span>{formatBytes(runSnapshot.progress.bytesCompleted)} of {formatBytes(runSnapshot.progress.bytesTotal)}</span><span>{runSnapshot.counts.completed} completed · {runSnapshot.counts.failed} failed · {runSnapshot.counts.cancelled} cancelled</span></div>
            </div>
          ) : <EmptyRow compact title="No active run" description="Start a selected source reel to see typed progress here." />}
          <div className="youtube-run-actions">
            <Button type="button" variant="primary" onClick={() => void handleStart()} loading={isStarting} loadingLabel="Starting" disabled={!scan || selectedCount === 0 || !outputDir.trim() || !helperReady || !acknowledged || activeRun}>
              <Play aria-hidden="true" />
              Start selected ({selectedCount})
            </Button>
            <Button type="button" variant="outline" onClick={() => void handleCancel()} loading={isCancelling} loadingLabel="Cancelling" disabled={!activeRun}>
              <Square aria-hidden="true" />
              Cancel run
            </Button>
          </div>
          {runSnapshot?.error ? <div className="youtube-run-error" role="alert"><AlertTriangle aria-hidden="true" /><span>{runSnapshot.error.message}</span></div> : null}
          {runSnapshot && runSnapshot.warnings.length > 0 ? <div className="youtube-run-warnings" role="status"><AlertTriangle aria-hidden="true" /><span>{runSnapshot.warnings[0]?.message}</span></div> : null}
          <div className="youtube-run-note"><RefreshCw aria-hidden="true" /><span>Progress is event-driven. Route reloads reconcile the active or most-recent revision; the view never runs a polling loop.</span></div>
        </Panel>
      </div>
    </div>
  );
}
