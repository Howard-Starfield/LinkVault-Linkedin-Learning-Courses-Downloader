import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import {
  CircleHelp,
  Folder,
  History,
  PanelLeft,
  Play,
  RotateCcw,
  Settings,
  SunMedium,
  Trash2,
  X
} from "lucide-react";
import { IconBrandLinkedin, IconMovie, IconTool } from "@tabler/icons-react";
import linkvaultLogo from "./assets/linkvault-wordmark.svg";
import {
  ActivityEventRow,
  Button,
  Checkbox,
  DataTable,
  DataTableHeader,
  DataTableRow,
  Dialog,
  EmptyRow,
  Field,
  IconButton,
  Input,
  Panel,
  Popover,
  Progress,
  Select,
  SidebarItem,
  StatusBadge,
  SummaryChip,
  Switch,
  Textarea,
  Tooltip,
  guardedToast
} from "./components/primitives";

type ParsedCourse = {
  original: string;
  normalized_url: string;
  slug: string;
  quiz_urls: string[];
  assessment_urns: string[];
};

type QueuedDownloadJob = {
  id: string;
  course_slug: string;
  source_url: string;
  status: string;
  thumbnail_url?: string | null;
  selected_quality?: string;
  output_dir?: string;
  updated_at?: number;
  artifact_counts?: ArtifactProgressCounts;
};

type ArtifactProgressCounts = {
  total: number;
  completed: number;
  failed: number;
  cancelled: number;
  active: number;
  pending: number;
  skipped: number;
  video_total: number;
  video_completed: number;
  subtitle_total: number;
  subtitle_completed: number;
  quiz_total?: number;
  quiz_completed?: number;
  exercise_total: number;
  exercise_completed: number;
};

type PersistedJobEvent = {
  id: number;
  job_id: string;
  event_type: string;
  message: string;
  created_at: number;
};

type ActivityRow = [time: string, label: string, tone?: string];

type StartDownloadResponse = {
  jobs: QueuedDownloadJob[];
};

type StartDownloadRequest = {
  courseUrls: string;
  outputDir: string;
  selectedQuality: string;
  delaySeconds: number;
  browserSource: string;
  downloadVideos: boolean;
  downloadExercises: boolean;
  downloadSubtitles: boolean;
  downloadQuizzes: boolean;
};

type ProcessQueuedDownloadResponse = {
  processed: boolean;
  completed_artifacts: number;
  failed_artifacts: number;
  cancelled_artifacts: number;
};

type CancelDownloadResponse = {
  cancellation_requested: boolean;
};

type SavedDownloadPreferences = {
  outputDir: string;
  selectedQuality: string;
  delaySeconds: number;
  browserSource: string;
  downloadVideos: boolean;
  downloadExercises: boolean;
  downloadSubtitles: boolean;
  downloadQuizzes?: boolean;
};

type BootstrapState = {
  default_resolution: string;
  browser_sources: string[];
  stores_plaintext_tokens_in_sqlite: boolean;
  has_saved_token: boolean;
  saved_download_preferences: SavedDownloadPreferences | null;
  persisted_jobs: QueuedDownloadJob[];
  recent_events: PersistedJobEvent[];
};

type PreviewCourseUrlError =
  | { type: "empty" }
  | { type: "notLinkedInLearning"; line: number }
  | { type: "missingSlug"; line: number }
  | { type: "invalidUrl"; line: number };

const SIDEBAR_MIN_WIDTH = 208;
const SIDEBAR_MAX_WIDTH = 320;
const SIDEBAR_DEFAULT_WIDTH = 220;
const SIDEBAR_WIDTH_STORAGE_KEY = "linkvault.sidebarWidth";
const SIDEBAR_COLLAPSED_STORAGE_KEY = "linkvault.sidebarCollapsed";

function clampSidebarWidth(width: number) {
  return Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH);
}

export default function App() {
  const [courseUrls, setCourseUrls] = useState("");
  const [folder, setFolder] = useState("");
  const [token, setToken] = useState("");
  const [resolution, setResolution] = useState("720");
  const [browserSource, setBrowserSource] = useState("Chrome");
  const [browserSources, setBrowserSources] = useState(["Chrome", "Edge", "Firefox"]);
  const [delaySeconds, setDelaySeconds] = useState(0);
  const [downloadVideos, setDownloadVideos] = useState(true);
  const [downloadExercises, setDownloadExercises] = useState(true);
  const [downloadSubtitles, setDownloadSubtitles] = useState(true);
  const [downloadQuizzes, setDownloadQuizzes] = useState(true);
  const [parsedCourses, setParsedCourses] = useState<ParsedCourse[]>([]);
  const [hasSavedToken, setHasSavedToken] = useState(false);
  const [isValidatingToken, setIsValidatingToken] = useState(false);
  const [isProcessingDownload, setIsProcessingDownload] = useState(false);
  const [isCancellingDownload, setIsCancellingDownload] = useState(false);
  const [isSavingSettings, setIsSavingSettings] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isHelpOpen, setIsHelpOpen] = useState(false);
  const [queuedJobs, setQueuedJobs] = useState<QueuedDownloadJob[]>([]);
  const [persistedEvents, setPersistedEvents] = useState<PersistedJobEvent[]>([]);
  const [processingSummary, setProcessingSummary] = useState<ProcessQueuedDownloadResponse | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT_WIDTH);
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(false);
  const [isDraggingSidebar, setIsDraggingSidebar] = useState(false);
  const sidebarDragStart = useRef({ x: 0, width: SIDEBAR_DEFAULT_WIDTH });
  const sidebarDragCleanup = useRef<(() => void) | null>(null);
  const wasSettingsOpen = useRef(false);

  useEffect(() => {
    refreshBootstrapState();
  }, []);

  useEffect(() => {
    const storedWidth = Number(window.localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY));
    if (Number.isFinite(storedWidth)) {
      setSidebarWidth(clampSidebarWidth(storedWidth));
    }
    setIsSidebarCollapsed(window.localStorage.getItem(SIDEBAR_COLLAPSED_STORAGE_KEY) === "true");
  }, []);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(sidebarWidth));
  }, [sidebarWidth]);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_COLLAPSED_STORAGE_KEY, String(isSidebarCollapsed));
  }, [isSidebarCollapsed]);

  useEffect(() => {
    if (wasSettingsOpen.current && !isSettingsOpen) {
      window.requestAnimationFrame(() => {
        document.querySelector<HTMLElement>('[aria-label="Open settings"]')?.focus();
      });
    }
    wasSettingsOpen.current = isSettingsOpen;
  }, [isSettingsOpen]);

  function startSidebarResize(event: ReactMouseEvent<HTMLButtonElement>) {
    if (isSidebarCollapsed) return;
    sidebarDragStart.current = { x: event.clientX, width: sidebarWidth };
    setIsDraggingSidebar(true);
    sidebarDragCleanup.current?.();

    function handleMouseMove(moveEvent: MouseEvent) {
      const nextWidth = sidebarDragStart.current.width + moveEvent.clientX - sidebarDragStart.current.x;
      setSidebarWidth(clampSidebarWidth(nextWidth));
    }

    function stopDragging() {
      setIsDraggingSidebar(false);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", stopDragging);
      sidebarDragCleanup.current = null;
    }

    document.body.style.cursor = "ew-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", stopDragging);
    sidebarDragCleanup.current = stopDragging;
    event.preventDefault();
  }

  useEffect(() => {
    return () => {
      sidebarDragCleanup.current?.();
    };
  }, []);

  async function refreshBootstrapState() {
    if (!isTauriRuntime()) {
      const previewPreferences = readPreviewPreferences();
      if (previewPreferences) {
        applyDownloadPreferences(previewPreferences);
      }
      const previewState = getBrowserPreviewState();
      if (previewState) {
        setQueuedJobs(previewState.jobs);
        setHasSavedToken(hasPreviewSavedToken());
        setPersistedEvents(previewState.events);
      }
      return;
    }

    try {
      const state = await invoke<BootstrapState>("bootstrap_state");
      setBrowserSources(state.browser_sources.length > 0 ? state.browser_sources : browserSources);
      const preferences = state.saved_download_preferences;
      if (preferences) {
        applyDownloadPreferences(preferences);
      } else if (state.default_resolution) {
        setResolution(String(state.default_resolution).replace("P", ""));
      }

      if (state.persisted_jobs.length > 0) {
        setQueuedJobs(state.persisted_jobs);
      } else {
        setQueuedJobs([]);
      }
      setHasSavedToken(state.has_saved_token);
      setPersistedEvents(state.recent_events ?? []);
    } catch {
      // Browser-only Vite previews do not expose Tauri commands.
      const previewState = getBrowserPreviewState();
      if (previewState) {
        setQueuedJobs(previewState.jobs);
        setHasSavedToken(hasPreviewSavedToken());
        setPersistedEvents(previewState.events);
      }
    }
  }

  function markNextQueuedJobActiveForLiveStats() {
    setQueuedJobs((jobs) => {
      const queuedIndex = jobs.findIndex((job) => job.status === "queued");
      if (queuedIndex < 0) return jobs;
      return jobs.map((job, index) => (
        index === queuedIndex
          ? { ...job, status: "active", updated_at: Math.floor(Date.now() / 1000) }
          : job
      ));
    });
  }

  function currentDownloadPreferences(): SavedDownloadPreferences {
    return {
      outputDir: folder,
      selectedQuality: resolution,
      delaySeconds,
      browserSource,
      downloadVideos,
      downloadExercises,
      downloadSubtitles,
      downloadQuizzes
    };
  }

  function applyDownloadPreferences(preferences: SavedDownloadPreferences) {
    setFolder(preferences.outputDir);
    setResolution(preferences.selectedQuality);
    setDelaySeconds(preferences.delaySeconds);
    setBrowserSource(preferences.browserSource);
    setDownloadVideos(preferences.downloadVideos);
    setDownloadExercises(preferences.downloadExercises);
    setDownloadSubtitles(preferences.downloadSubtitles);
    setDownloadQuizzes(preferences.downloadQuizzes ?? true);
  }

  const canStart = useMemo(
    () => courseUrls.trim().length > 0 && folder.trim().length > 0 && (token.trim().length > 0 || hasSavedToken) && !isProcessingDownload,
    [courseUrls, folder, token, hasSavedToken, isProcessingDownload]
  );

  const queueCounts = useMemo(
    () =>
      queuedJobs.reduce(
        (counts, job) => {
          counts[job.status] = (counts[job.status] ?? 0) + 1;
          return counts;
        },
        {} as Record<string, number>
      ),
    [queuedJobs]
  );

  const liveQueueJobs = queuedJobs.filter((job) => shouldShowInLiveQueue(job.status));
  const completedJobs = completedCourseJobs(queuedJobs);
  const displayedQueueJobs = liveQueueJobs;
  const persistedActivityEvents = coalesceActivityEvents(persistedEvents);

  const queueSummary = queuedJobs.length > 0
    ? ([
        queueCounts.active ? `${queueCounts.active} active` : null,
        queueCounts.queued ? `${queueCounts.queued} queued` : null,
        queueCounts.failed ? `${queueCounts.failed} failed` : null,
        queueCounts.cancelled ? `${queueCounts.cancelled} cancelled` : null
      ].filter(Boolean).join(" - ") || "0 active")
    : "No persisted jobs";

  const activityEvents = processingSummary?.processed
    ? [
        [
          "Now",
          `Processed queued job: ${processingSummary.completed_artifacts} completed, ${processingSummary.failed_artifacts} failed, ${processingSummary.cancelled_artifacts} cancelled`,
          processingSummary.failed_artifacts > 0 ? "danger" : "success"
        ],
        ...persistedActivityEvents
      ] satisfies ActivityRow[]
    : persistedActivityEvents;

  const activitySummary = {
    active: queueCounts.active ?? 0,
    completed: queueCounts.completed ?? 0,
    failed: (queueCounts.failed ?? 0) + (queueCounts.cancelled ?? 0)
  };

  async function clearFailedQueueItems() {
    if (activitySummary.failed === 0) return;
    try {
      const state = await clearFailedDownloadJobs();
      setQueuedJobs(state.persisted_jobs);
      setPersistedEvents(state.recent_events ?? []);
      setHasSavedToken(state.has_saved_token);
      setProcessingSummary(null);
      toast.info("Failed queue cleared", {
        description: "Failed and cancelled items were removed from the queue."
      });
    } catch (error) {
      toast.error("Clear failed queue failed", { description: String(error) });
    }
  }

  async function validateUrls() {
    if (!courseUrls.trim()) {
      toast.warning("Course URL required", { description: "Paste at least one LinkedIn Learning course URL." });
      setParsedCourses([]);
      return [];
    }

    try {
      const parsed = await parseLinkedInCourseUrls(courseUrls);
      setParsedCourses(parsed);
      toast.success("Course URLs validated", {
        description: `${parsed.length} LinkedIn Learning course${parsed.length === 1 ? "" : "s"} ready to queue.`
      });
      return parsed;
    } catch (error) {
      setParsedCourses([]);
      toast.error("Invalid course URL", { description: String(error) });
      return [];
    }
  }

  async function startDownload() {
    const parsed = await validateUrls();
    if (parsed.length === 0) return;
    const enteredToken = token.trim();
    if (enteredToken) {
      setIsValidatingToken(true);
      try {
        await saveLinkedInToken(enteredToken);
        setHasSavedToken(true);
        setToken("");
      } catch (error) {
        toast.error("Token validation failed", { description: String(error) });
        setIsValidatingToken(false);
        return;
      }
      setIsValidatingToken(false);
    } else if (!hasSavedToken) {
      toast.warning("LinkedIn token required", { description: "Paste li_at once; LinkVault will save it for future launches." });
      return;
    }
    try {
      setIsProcessingDownload(true);
      setProcessingSummary(null);
      const response = await startDownloadJobs({
        courseUrls,
        outputDir: folder,
        selectedQuality: resolution,
        delaySeconds,
        browserSource,
        downloadVideos,
        downloadExercises,
        downloadSubtitles,
        downloadQuizzes
      });
      setQueuedJobs(response.jobs);
      setParsedCourses([]);
      toast.success("Download queued", {
        description: `${response.jobs.length} LinkedIn course${response.jobs.length === 1 ? "" : "s"} persisted to the local queue.`
      });

      const processResponse = await processQueuedDownloadWithLiveRefresh(() =>
        processNextQueuedDownloadWithSavedToken()
      );

      setProcessingSummary(processResponse);
      await refreshBootstrapState();
      if (processResponse.processed) {
        showProcessedDownloadToast(processResponse);
      } else {
        toast.info("No queued download to process", {
          description: "The local queue did not contain a pending LinkedIn course."
        });
      }
    } catch (error) {
      await refreshBootstrapState();
      toast.error("Download processing failed", { description: String(error) });
    } finally {
      setIsProcessingDownload(false);
    }
  }

  async function processQueuedDownloadWithLiveRefresh(processOperation: () => Promise<ProcessQueuedDownloadResponse>) {
    markNextQueuedJobActiveForLiveStats();
    let settled = false;
    const processPromise = processOperation().finally(() => {
      settled = true;
    });

    void sleep(50).then(() => {
      if (!settled) void refreshBootstrapState();
    });

    while (!settled) {
      await sleep(150);
      if (!settled) {
        await refreshBootstrapState();
      }
    }

    const response = await processPromise;
    await refreshBootstrapState();
    return response;
  }

  async function clearToken() {
    try {
      await clearSavedLinkedInToken();
      setHasSavedToken(false);
      setToken("");
      toast.info("Saved token cleared", {
        description: "Paste a LinkedIn li_at token before the next download."
      });
    } catch (error) {
      toast.error("Token clear failed", { description: String(error) });
    }
  }

  async function saveSettings() {
    if (!folder.trim()) {
      toast.warning("Download folder required", { description: "Choose a default folder before saving settings." });
      return;
    }

    setIsSavingSettings(true);
    const startedAt = Date.now();
    try {
      const preferences = await saveDownloadPreferences(currentDownloadPreferences());
      applyDownloadPreferences(preferences);
      toast.success("Settings saved", {
        description: "Download defaults will be restored the next time LinkVault opens."
      });
    } catch (error) {
      toast.error("Settings save failed", { description: String(error) });
    } finally {
      const remaining = 320 - (Date.now() - startedAt);
      if (remaining > 0) {
        await new Promise((resolve) => setTimeout(resolve, remaining));
      }
      setIsSavingSettings(false);
    }
  }

  async function cancelDownload() {
    if (!isProcessingDownload) return;
    setIsCancellingDownload(true);
    try {
      const response = await invoke<CancelDownloadResponse>("cancel_active_download");
      if (response.cancellation_requested) {
        toast.info("Cancellation requested", {
          description: "The active job will stop at the next safe cancellation boundary."
        });
      }
      await refreshBootstrapState();
    } catch (error) {
      toast.error("Cancellation failed", { description: String(error) });
    } finally {
      setIsCancellingDownload(false);
    }
  }

  async function retryDownloadJob(job: QueuedDownloadJob) {
    if (job.status !== "failed") return;
    const enteredToken = token.trim();
    if (!enteredToken && !hasSavedToken) {
      toast.warning("LinkedIn token required", {
        description: "Paste li_at once before retrying this failed course."
      });
      return;
    }

    try {
      setIsProcessingDownload(true);
      setProcessingSummary(null);
      if (enteredToken) {
        await saveLinkedInToken(enteredToken);
        setHasSavedToken(true);
        setToken("");
      }
      await retryFailedDownloadJob(job.id);
      setQueuedJobs((jobs) =>
        jobs.map((candidate) =>
          candidate.id === job.id
            ? { ...candidate, status: "queued", artifact_counts: emptyArtifactCounts() }
            : candidate
        )
      );
      toast.info("Retry queued", { description: courseDisplayName(job) });

      const processResponse = await processQueuedDownloadWithLiveRefresh(() =>
        processNextQueuedDownloadWithSavedToken()
      );

      setProcessingSummary(processResponse);
      await refreshBootstrapState();
      if (processResponse.processed) {
        showProcessedDownloadToast(processResponse);
      } else {
        toast.info("No queued download to process", {
          description: "The retry was queued, but no pending course was available."
        });
      }
    } catch (error) {
      await refreshBootstrapState();
      toast.error("Retry failed", { description: String(error) });
    } finally {
      setIsProcessingDownload(false);
    }
  }

  async function openCompletedFolder(job: QueuedDownloadJob) {
    const outputDir = job.output_dir?.trim();
    if (!outputDir) {
      toast.warning("Folder unavailable", { description: "This completed course does not have a saved output folder." });
      return;
    }

    try {
      const opened = await openDownloadFolder(outputDir);
      if (opened) {
        toast.success("Folder opened", { description: outputDir });
      }
    } catch (error) {
      toast.error("Open folder failed", { description: String(error) });
    }
  }

  async function browseDownloadFolder() {
    if (!isTauriRuntime()) {
      guardedToast("Folder picker unavailable in preview", "The native folder picker is available in the Tauri desktop runtime.");
      return;
    }

    try {
      const selectedFolder = await open({
        directory: true,
        multiple: false,
        defaultPath: folder || undefined
      });
      if (typeof selectedFolder === "string" && selectedFolder.trim()) {
        setFolder(selectedFolder);
        toast.success("Download folder selected", { description: selectedFolder });
      }
    } catch (error) {
      toast.error("Folder picker failed", { description: String(error) });
    }
  }

  return (
    <>
    <div
      className="lv-shell"
      data-sidebar-dragging={isDraggingSidebar || undefined}
      data-sidebar-state={isSidebarCollapsed ? "collapsed" : "expanded"}
      style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties}
    >
      <aside className="lv-sidebar" aria-label="Primary navigation">
        <div className="lv-sidebar-brand border-b border-sidebar-border">
          <div className="lv-sidebar-trigger-wrap">
            <Tooltip label="Toggle sidebar">
              <IconButton className="lv-sidebar-trigger" aria-label="Toggle sidebar" aria-expanded={!isSidebarCollapsed} onClick={() => setIsSidebarCollapsed(true)}>
                <PanelLeft aria-hidden="true" className="h-4 w-4" />
              </IconButton>
            </Tooltip>
          </div>
          <div className="lv-brand-logo" aria-label="LinkVault Course Downloader">
            <img src={linkvaultLogo} alt="" width={470} height={117} />
          </div>
          <h1 className="sr-only">LinkVault</h1>
        </div>

        <nav className="grid flex-1 content-start gap-1 px-3 py-3 text-xs">
          <SidebarItem active icon={<IconBrandLinkedin aria-hidden="true" size={18} />}>LinkedIn Courses</SidebarItem>
          <SidebarItem disabled title="Unavailable in the LinkedIn Learning MVP" icon={<IconMovie aria-hidden="true" size={18} />}>Generic Video</SidebarItem>
          <SidebarItem icon={<IconTool aria-hidden="true" size={18} />}>Tools</SidebarItem>
          <SidebarItem icon={<History aria-hidden="true" />}>History</SidebarItem>
          <div className="mt-7 flex items-center justify-between border-t border-sidebar-border pt-6 text-xs text-sidebar-muted">
            <span>LinkedIn Scraper</span>
            <span className="rounded-full border border-sidebar-border px-2 py-0.5 text-[11px]">Coming Soon</span>
          </div>
          <SidebarItem className="mt-5" icon={<Settings aria-hidden="true" />} aria-label="Open settings" onClick={() => setIsSettingsOpen(true)}>Settings</SidebarItem>
        </nav>

        <div className="flex items-center justify-between py-4 pl-6 pr-0 text-xs text-sidebar-muted">
          <span>v1.2.0</span>
          <div className="flex items-center gap-2">
            <SunMedium aria-hidden="true" className="h-4 w-4" />
            <Popover
              label="LinkVault help"
              open={isHelpOpen}
              onOpenChange={setIsHelpOpen}
              side="right"
              align="end"
              trigger={
                <Tooltip label="Open help">
                  <IconButton aria-label="Open help" aria-expanded={isHelpOpen} onClick={() => setIsHelpOpen((open) => !open)}>
                    <CircleHelp aria-hidden="true" className="h-4 w-4" />
                  </IconButton>
                </Tooltip>
              }
            >
              <div className="text-xs font-semibold text-muted-strong">LinkedIn Courses MVP</div>
              <p className="mt-2 text-xs leading-5 text-muted">
                Generic Video and LinkedIn Scraper are visible for context only. Course downloads use a saved local LinkedIn session after you paste li_at once.
              </p>
            </Popover>
          </div>
        </div>
        <button
          type="button"
          className="lv-sidebar-rail"
          aria-label="Resize sidebar"
          tabIndex={-1}
          onMouseDown={startSidebarResize}
        />
      </aside>
      <main className="lv-main">
        <div className="lv-content">
          <div className="lv-workspace">
            <Panel className="command-panel">
              <div className="section-heading command-section-heading">
                <button
                  type="button"
                  className="lv-sidebar-reopen"
                  aria-label="Show sidebar"
                  aria-hidden={!isSidebarCollapsed}
                  tabIndex={isSidebarCollapsed ? 0 : -1}
                  onClick={() => setIsSidebarCollapsed(false)}
                >
                  <PanelLeft aria-hidden="true" className="h-4 w-4" />
                </button>
                <div className="min-w-0">
                  <h3>Linkedin Course</h3>
                  <p>Paste LinkedIn Learning course URLs and choose what to download.</p>
                </div>
                <div className="ml-auto flex shrink-0 items-center gap-2">
                  <StatusBadge tone={hasSavedToken ? "success" : "muted"}>
                    {hasSavedToken ? "Saved session active" : "Session required"}
                  </StatusBadge>
                </div>
              </div>

              <div className="command-grid">
                <Field label="Course URLs">
                  <div className="course-url-field compact-url-field">
                    <Textarea
                      value={courseUrls}
                      onChange={(event) => {
                        setCourseUrls(event.target.value);
                        setParsedCourses([]);
                      }}
                      onBlur={validateUrls}
                      placeholder="One course URL per line"
                      spellCheck={false}
                      className="course-url-textarea"
                      aria-label="Course URLs"
                    />
                  </div>
                </Field>

                <div className="compact-field-row">
                  <Field label="Download folder">
                    <div className="field-action-grid">
                      <Input value={folder} onChange={(event) => setFolder(event.target.value)} aria-label="Download folder" />
                      <Button type="button" onClick={browseDownloadFolder}>
                        <Folder aria-hidden="true" className="h-3.5 w-3.5" />
                        Browse
                      </Button>
                    </div>
                  </Field>

                  <Field label="Token cookie">
                    <div className="field-action-grid token-grid">
                      <Input
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        placeholder={hasSavedToken ? "Saved token available" : "Paste your LinkedIn li_at cookie value"}
                        type="password"
                        aria-label="LinkedIn li_at token"
                      />
                      <Button type="button" onClick={clearToken}>
                        <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                        Clear
                      </Button>
                    </div>
                  </Field>
                </div>

                <div className="option-row">
                  <Field label="Quality">
                    <Select value={resolution} onChange={(event) => setResolution(event.target.value)} aria-label="Video resolution">
                      <option value="1080">1080 (Best)</option>
                      <option value="720">720 (High)</option>
                      <option value="540">540 (Medium)</option>
                      <option value="360">360 (Low)</option>
                    </Select>
                  </Field>
                  <Field label="Delay">
                    <Input
                      value={delaySeconds}
                      type="number"
                      min={0}
                      onChange={(event) => setDelaySeconds(Number(event.target.value))}
                      aria-label="Delay seconds"
                    />
                  </Field>
                  <div className="download-toggles">
                    <Checkbox checked={downloadVideos} onChange={(event) => setDownloadVideos(event.target.checked)} label="Videos" />
                    <Checkbox checked={downloadExercises} onChange={(event) => setDownloadExercises(event.target.checked)} label="Exercises" />
                    <Checkbox checked={downloadSubtitles} onChange={(event) => setDownloadSubtitles(event.target.checked)} label="Subtitles" />
                    <Checkbox checked={downloadQuizzes} onChange={(event) => setDownloadQuizzes(event.target.checked)} label="Quizzes" />
                  </div>
                  <div className="command-actions">
                    <Button type="button" variant="primary" onClick={startDownload} disabled={!canStart || isValidatingToken || isProcessingDownload}>
                      <Play aria-hidden="true" className="h-3.5 w-3.5" />
                      {isValidatingToken ? "Validating" : isProcessingDownload ? "Processing" : "Start Download"}
                    </Button>
                    <Button type="button" variant="outline" onClick={cancelDownload} disabled={!isProcessingDownload || isCancellingDownload}>
                      <X aria-hidden="true" className="h-3.5 w-3.5" />
                      {isCancellingDownload ? "Cancelling" : "Cancel"}
                    </Button>
                  </div>
                </div>
              </div>
            </Panel>

            <Panel className="table-panel">
              <div className="table-panel-header">
                <h3>Download Queue</h3>
                <div className="table-panel-header-status">
                  <span>{queuedJobs.length > 0 ? queueSummary : parsedCourses.length > 0 ? `${parsedCourses.length} validated` : "0 active"}</span>
                  {activitySummary.failed > 0 ? (
                    <button
                      type="button"
                      className="queue-clear-button"
                      aria-label="Clear failed queue items"
                      onClick={clearFailedQueueItems}
                    >
                      Clear
                    </button>
                  ) : null}
                </div>
              </div>
              <DownloadQueueTable jobs={displayedQueueJobs} parsedCourses={parsedCourses} hasPersistedJobs={queuedJobs.length > 0} onRetry={retryDownloadJob} />
            </Panel>
          </div>

          <Panel className="lv-activity">
            <div className="activity-summary-grid">
              <ActivitySummaryChip label="Active" value={activitySummary.active} tone="primary" />
              <ActivitySummaryChip label="Completed" value={activitySummary.completed} tone="success" />
              <ActivitySummaryChip label="Failed" value={activitySummary.failed} tone="danger" />
            </div>
            <div className="activity-section">
              <div className="activity-section-header">
                <h4>Recent Activity</h4>
              </div>
              <ActivityLog events={activityEvents} />
            </div>
            <div className="activity-section completed-section">
              <div className="activity-section-header">
                <h4>Completed</h4>
              </div>
              <CompletedDownloadsTable jobs={completedJobs} onOpenFolder={openCompletedFolder} />
            </div>
          </Panel>
        </div>
      </main>
    </div>
    <Dialog
      open={isSettingsOpen}
      onOpenChange={setIsSettingsOpen}
      title="LinkVault settings"
      description="Save downloader defaults, session behavior, and artifact options without storing plaintext LinkedIn tokens."
    >
      <div className="settings-grid">
        <section className="settings-section">
          <div className="settings-section-title">Download defaults</div>
          <Field label="Download folder">
            <div className="field-action-grid">
              <Input value={folder} onChange={(event) => setFolder(event.target.value)} aria-label="Settings download folder" />
              <Button type="button" onClick={browseDownloadFolder}>
                <Folder aria-hidden="true" className="h-3.5 w-3.5" />
                Browse
              </Button>
            </div>
          </Field>
          <div className="settings-two-column">
            <Field label="Video quality">
              <Select value={resolution} onChange={(event) => setResolution(event.target.value)} aria-label="Settings video quality">
                <option value="1080">1080 (Best)</option>
                <option value="720">720 (High)</option>
                <option value="540">540 (Medium)</option>
                <option value="360">360 (Low)</option>
              </Select>
            </Field>
            <Field label="Delay seconds">
              <Input value={delaySeconds} type="number" min={0} onChange={(event) => setDelaySeconds(Number(event.target.value))} aria-label="Settings delay seconds" />
            </Field>
          </div>
          <Field label="Browser source">
            <Select value={browserSource} onChange={(event) => setBrowserSource(event.target.value)} aria-label="Settings browser source">
              {browserSources.map((source) => <option key={source} value={source}>{source}</option>)}
            </Select>
          </Field>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">Artifacts</div>
          <div className="settings-switch-list">
            <Switch checked={downloadVideos} onChange={(event) => setDownloadVideos(event.target.checked)} label="Download videos by default" />
            <Switch checked={downloadExercises} onChange={(event) => setDownloadExercises(event.target.checked)} label="Download exercise files" />
            <Switch checked={downloadSubtitles} onChange={(event) => setDownloadSubtitles(event.target.checked)} label="Download subtitles" />
            <Switch checked={downloadQuizzes} onChange={(event) => setDownloadQuizzes(event.target.checked)} label="Extract quiz questions" />
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">LinkedIn session</div>
          <div className="settings-row">
            <span>Saved token</span>
            <span className={hasSavedToken ? "text-success" : "text-muted"}>{hasSavedToken ? "Available" : "Not saved"}</span>
          </div>
          <div className="settings-row">
            <span>Plaintext token storage</span>
            <span className="text-success">Disabled</span>
          </div>
          <Button type="button" variant="outline" onClick={clearToken} disabled={!hasSavedToken && !token}>
            <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
            Clear saved token
          </Button>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">Application</div>
          <div className="settings-row">
            <span>Theme</span>
            <span>Jan dark</span>
          </div>
          <div className="settings-row">
            <span>Version</span>
            <span>v1.2.0</span>
          </div>
        </section>

        <div className="settings-actions">
          <Button type="button" variant="ghost" onClick={() => setIsSettingsOpen(false)}>Close</Button>
          <Button type="button" variant="primary" onClick={saveSettings} loading={isSavingSettings} loadingLabel="Saving">
            Save settings
          </Button>
        </div>
      </div>
    </Dialog>
    </>
  );
}

function isTerminalJob(status: string) {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function shouldShowInLiveQueue(status: string) {
  return status !== "completed" && status !== "cancelled";
}

function completedCourseJobs(jobs: QueuedDownloadJob[]) {
  const latestByCourse = new Map<string, QueuedDownloadJob>();
  for (const job of jobs) {
    if (job.status !== "completed") continue;
    const key = job.course_slug || job.output_dir || job.id;
    const existing = latestByCourse.get(key);
    if (!existing || (job.updated_at ?? 0) >= (existing.updated_at ?? 0)) {
      latestByCourse.set(key, job);
    }
  }
  return [...latestByCourse.values()].sort((first, second) => (second.updated_at ?? 0) - (first.updated_at ?? 0));
}

function formatEventTime(timestamp: number) {
  if (!timestamp) return "--:--";
  return new Date(timestamp * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false
  });
}

function eventTone(eventType: string) {
  if (eventType.includes("failed")) return "danger";
  if (eventType.includes("completed") || eventType.includes("extracted")) return "success";
  if (eventType.includes("cancelled")) return "muted";
  return "primary";
}

function activityDotClass(tone?: string) {
  if (tone === "danger") return "bg-danger";
  if (tone === "success") return "bg-success";
  if (tone === "muted") return "bg-muted";
  return "bg-primary";
}

function coalesceActivityEvents(events: PersistedJobEvent[]): ActivityRow[] {
  const rows: ActivityRow[] = [];
  let groupedType: string | null = null;
  let groupedRows: ActivityRow[] = [];

  function flushGroup() {
    if (!groupedType || groupedRows.length === 0) return;
    if (groupedRows.length < 8) {
      rows.push(...groupedRows);
      groupedType = null;
      groupedRows = [];
      return;
    }

    const [time, , tone] = groupedRows[0];
    const label = groupedType === "artifact.active"
      ? `${groupedRows.length} artifact downloads started`
      : `${groupedRows.length} artifacts completed`;
    rows.push([time, label, tone]);
    groupedType = null;
    groupedRows = [];
  }

  for (const event of events.slice(0, 80)) {
    if (event.event_type === "artifact.source.diagnostic") continue;

    const groupable = event.event_type === "artifact.active" || event.event_type === "artifact.completed";
    if (groupable) {
      if (groupedType === event.event_type) {
        groupedRows.push([formatEventTime(event.created_at), event.message, eventTone(event.event_type)]);
        continue;
      }
      flushGroup();
      groupedType = event.event_type;
      groupedRows = [[formatEventTime(event.created_at), event.message, eventTone(event.event_type)]];
      continue;
    }

    flushGroup();
    rows.push([formatEventTime(event.created_at), event.message, eventTone(event.event_type)]);
  }

  flushGroup();
  return rows;
}

function ActivitySummaryChip({ label, value, tone }: { label: string; value: number; tone: string }) {
  return <SummaryChip label={label} value={value} dotClassName={activityDotClass(tone)} />;
}

function ActivityLog({ events }: { events: ActivityRow[] }) {
  return (
    <ol className="activity-list">
      {events.length > 0 ? events.map(([time, label, tone]) => (
        <ActivityEventRow key={`${time}-${label}`} time={time} label={label} dotClassName={activityDotClass(tone)} />
      )) : (
        <li className="activity-empty-row">No persisted activity yet.</li>
      )}
    </ol>
  );
}

function DownloadQueueTable({
  jobs,
  parsedCourses,
  hasPersistedJobs,
  onRetry
}: {
  jobs: QueuedDownloadJob[];
  parsedCourses: ParsedCourse[];
  hasPersistedJobs: boolean;
  onRetry: (job: QueuedDownloadJob) => void | Promise<void>;
}) {
  return (
    <DataTable className="queue-table">
      <DataTableHeader>
        <span>Status</span>
        <span>Course</span>
        <span>Progress</span>
      </DataTableHeader>
      {jobs.length > 0 ? (
        jobs.map((job) => <QueueJobRow key={job.id} job={job} onRetry={onRetry} />)
      ) : parsedCourses.length > 0 ? (
        parsedCourses.map((course, index) => <ValidatedQueueRow key={`${course.slug}-${index}`} course={course} />)
      ) : (
        <EmptyRow
          title="No active downloads"
          description={hasPersistedJobs ? "Finished courses are in Completed. Failed jobs stay here until handled." : "Active jobs and items needing attention appear here after Start Download."}
        />
      )}
    </DataTable>
  );
}

function QueueJobRow({ job, onRetry }: { job: QueuedDownloadJob; onRetry: (job: QueuedDownloadJob) => void | Promise<void> }) {
  const counts = artifactCounts(job);
  const progress = courseOverallProgress(job, counts);
  const title = courseDisplayName(job);
  const queueLabel = queueCourseLabel(job, counts);

  return (
    <DataTableRow className="queue-table-row">
      <QueueStatusBadge job={job} title={title} onRetry={onRetry} />
      <div className="table-course-cell">
        {job.thumbnail_url ? <MiniCourseArt title={title} thumbnailUrl={job.thumbnail_url} /> : <span className={`course-status-mark ${activityDotClass(eventTone(job.status))}`} />}
        <div className="min-w-0">
          <div className="truncate font-medium" title={title}>{queueLabel}</div>
          <div className="truncate text-soft" title={job.source_url}>{filesSummaryText(counts, job.status)}</div>
        </div>
      </div>
      <div className="table-progress-cell">
        <Progress value={progress} />
        <span>{progress}%</span>
      </div>
    </DataTableRow>
  );
}

function ValidatedQueueRow({ course }: { course: ParsedCourse }) {
  const title = courseDisplayNameFromSlug(course.slug);
  return (
    <DataTableRow className="queue-table-row">
      <StatusBadge tone="primary" dotClassName="bg-primary">Validated</StatusBadge>
      <div className="table-course-cell">
        <span className="course-status-mark bg-primary" />
        <div className="min-w-0">
          <div className="truncate font-medium" title={title}>Ready to queue</div>
          <div className="truncate text-soft" title={course.normalized_url}>{course.normalized_url}</div>
        </div>
      </div>
      <span className="text-muted">Waiting</span>
    </DataTableRow>
  );
}

function QueueStatusBadge({ job, title, onRetry }: { job: QueuedDownloadJob; title: string; onRetry: (job: QueuedDownloadJob) => void | Promise<void> }) {
  return (
    <StatusBadge className={jobStatusBadgeClass(job.status)} dotClassName={activityDotClass(eventTone(job.status))}>
      <span>{jobStatusLabel(job.status)}</span>
      {job.status === "failed" ? (
        <button
          type="button"
          className="queue-status-retry"
          aria-label={`Retry ${title}`}
          onClick={() => onRetry(job)}
        >
          <RotateCcw aria-hidden="true" className="h-3.5 w-3.5" />
        </button>
      ) : null}
    </StatusBadge>
  );
}

function CompletedDownloadsTable({ jobs, onOpenFolder }: { jobs: QueuedDownloadJob[]; onOpenFolder: (job: QueuedDownloadJob) => void | Promise<void> }) {
  return (
    <DataTable className="completed-list completed-table">
      {jobs.length > 0 ? jobs.map((job) => <CompletedDownloadRow key={job.id} job={job} onOpenFolder={onOpenFolder} />) : (
        <EmptyRow compact title="No completed jobs" description="Finished courses will appear here after processing." />
      )}
    </DataTable>
  );
}

function CompletedDownloadRow({ job, onOpenFolder }: { job: QueuedDownloadJob; onOpenFolder: (job: QueuedDownloadJob) => void | Promise<void> }) {
  const counts = artifactCounts(job);
  const title = courseDisplayName(job);
  const statusText = job.status.charAt(0).toUpperCase() + job.status.slice(1);
  const time = formatEventTime(job.updated_at ?? 0);
  const outputDir = job.output_dir ?? "";

  return (
    <div className="completed-row">
      <span className={`status-dot ${activityDotClass(eventTone(job.status))}`} />
      <div className="min-w-0">
        <div className="truncate font-medium" title={title}>{title}</div>
        <div className="truncate text-soft" title={outputDir}>
          {statusText} - {time} - {filesSummaryText(counts, job.status)}
        </div>
      </div>
      <Button size="sm" variant="ghost" disabled={!outputDir} onClick={() => onOpenFolder(job)}>
        Open Folder
      </Button>
    </div>
  );
}

function MiniCourseArt({ title, thumbnailUrl }: { title: string; thumbnailUrl: string }) {
  return (
    <span className="mini-course-art" title={title}>
      <img src={thumbnailUrl} alt="" loading="lazy" referrerPolicy="no-referrer" />
    </span>
  );
}

function courseInitials(title: string) {
  const parts = title.split(/\s+/).filter(Boolean);
  const initials = parts.slice(0, 3).map((part) => part.charAt(0).toUpperCase()).join("");
  return initials || "LL";
}

function courseSubtitle(job: QueuedDownloadJob, counts: ArtifactProgressCounts) {
  return artifactSummaryText(counts, job.status);
}

function queueCourseLabel(job: QueuedDownloadJob, counts: ArtifactProgressCounts) {
  const subtitle = courseSubtitle(job, counts);
  if (subtitle.startsWith("Course ")) return subtitle.replace("Course", "Chapter");
  return courseDisplayName(job);
}

function courseOverallProgress(job: QueuedDownloadJob, counts: ArtifactProgressCounts) {
  return artifactProgressPercent(counts.completed + counts.failed + counts.cancelled, counts.total, job.status);
}

function artifactCounts(job: QueuedDownloadJob): ArtifactProgressCounts {
  return job.artifact_counts ?? {
    total: 0,
    completed: 0,
    failed: 0,
    cancelled: 0,
    active: 0,
    pending: 0,
    skipped: 0,
    video_total: 0,
    video_completed: 0,
    subtitle_total: 0,
    subtitle_completed: 0,
    quiz_total: 0,
    quiz_completed: 0,
    exercise_total: 0,
    exercise_completed: 0
  };
}

function artifactProgressPercent(completed: number, total: number, status: string) {
  if (total <= 0) return status === "completed" ? 100 : 0;
  return Math.max(0, Math.min(100, Math.round((completed / total) * 100)));
}

function artifactSummaryText(counts: ArtifactProgressCounts, status: string) {
  if (counts.total === 0) return status === "queued" ? "Waiting for artifact plan" : "No artifacts planned";
  const problemParts = [
    counts.failed > 0 ? `${counts.failed} failed` : null,
    counts.cancelled > 0 ? `${counts.cancelled} cancelled` : null
  ].filter(Boolean);
  const problemText = problemParts.length > 0 ? `, ${problemParts.join(", ")}` : "";
  return `${counts.completed} of ${counts.total} artifacts complete${problemText}`;
}

function filesSummaryText(counts: ArtifactProgressCounts, status: string) {
  if (counts.total === 0) return status === "queued" ? "Pending" : "0 files";
  const issues = [
    counts.failed > 0 ? `${counts.failed} failed` : null,
    counts.cancelled > 0 ? `${counts.cancelled} cancelled` : null
  ].filter(Boolean);
  const issueText = issues.length > 0 ? `, ${issues.join(", ")}` : "";
  if (status === "completed") return `${counts.completed} of ${counts.total} files${issueText}`;
  return `${counts.completed}/${counts.total} files${issueText}`;
}

function courseDisplayName(job: QueuedDownloadJob) {
  return courseDisplayNameFromSlug(job.course_slug);
}

function courseDisplayNameFromSlug(slug: string) {
  return slug
    .split("-")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ") || "LinkedIn Learning Course";
}

function jobStatusBadgeClass(status: string) {
  if (status === "completed") return "bg-success/10 text-success";
  if (status === "failed") return "bg-danger/10 text-danger";
  if (status === "cancelled") return "bg-muted/30 text-muted";
  if (status === "queued") return "bg-secondary text-muted-strong";
  return "bg-primary/15 text-primary";
}

function jobStatusLabel(status: string) {
  if (status === "active") return "Downloading";
  return status.charAt(0).toUpperCase() + status.slice(1);
}

function showProcessedDownloadToast(response: ProcessQueuedDownloadResponse) {
  const description = `${response.completed_artifacts} completed, ${response.failed_artifacts} failed, ${response.cancelled_artifacts} cancelled.`;
  if (response.failed_artifacts > 0 || response.cancelled_artifacts > 0) {
    toast.warning("Queued download processed with issues", { description });
    return;
  }

  toast.success("Queued download processed", { description });
}

function sleep(milliseconds: number) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

async function parseLinkedInCourseUrls(input: string) {
  try {
    return await invoke<ParsedCourse[]>("parse_linkedin_course_urls", { input });
  } catch (error) {
    if (isTauriRuntime()) {
      throw error;
    }
    return parseLinkedInCourseUrlsForPreview(input);
  }
}

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function saveLinkedInToken(token: string) {
  if (isTauriRuntime()) {
    return invoke<{ has_saved_token: boolean }>("save_li_at_token", { token });
  }

  setPreviewSavedToken(token.trim().length > 0);
  return { has_saved_token: token.trim().length > 0 };
}

async function clearSavedLinkedInToken() {
  if (isTauriRuntime()) {
    return invoke<{ has_saved_token: boolean }>("clear_saved_li_at_token");
  }

  setPreviewSavedToken(false);
  return { has_saved_token: false };
}

async function saveDownloadPreferences(preferences: SavedDownloadPreferences) {
  if (isTauriRuntime()) {
    return invoke<SavedDownloadPreferences>("save_download_preferences", { preferences });
  }

  writePreviewPreferences(preferences);
  return preferences;
}

async function startDownloadJobs(request: StartDownloadRequest) {
  if (isTauriRuntime()) {
    return invoke<StartDownloadResponse>("start_download_jobs", { request });
  }

  return startDownloadJobsForPreview(request);
}

async function retryFailedDownloadJob(jobId: string) {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("retry_failed_download_job", { jobId });
  }

  return retryFailedDownloadJobForPreview(jobId);
}

async function clearFailedDownloadJobs() {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("clear_failed_download_jobs");
  }

  const jobs = readPreviewJobs().filter((job) => job.status !== "failed" && job.status !== "cancelled");
  const events = readPreviewEvents();
  writePreviewState(jobs, events);
  return {
    persisted_jobs: jobs,
    recent_events: events,
    has_saved_token: hasPreviewSavedToken(),
    saved_download_preferences: readPreviewPreferences(),
    stores_plaintext_tokens_in_sqlite: false,
    browser_sources: ["Chrome", "Edge", "Firefox"],
    default_resolution: "P720"
  } satisfies BootstrapState;
}

async function openDownloadFolder(path: string) {
  if (isTauriRuntime()) {
    await openPath(path);
    return true;
  }

  guardedToast("Folder opener unavailable in preview", path);
  return false;
}

async function processNextQueuedDownloadWithSavedToken() {
  if (isTauriRuntime()) {
    return invoke<ProcessQueuedDownloadResponse>("process_next_queued_download_with_saved_token");
  }

  if (!hasPreviewSavedToken()) {
    throw new Error("Saved LinkedIn token is unavailable");
  }
  return processNextQueuedDownloadForPreview();
}

function parseLinkedInCourseUrlsForPreview(input: string): ParsedCourse[] {
  const courses: ParsedCourse[] = [];
  for (const [index, rawLine] of input.split(/\r?\n/).entries()) {
    const line = index + 1;
    const trimmed = rawLine.trim();
    if (!trimmed) continue;
    courses.push(parseLinkedInCourseUrlForPreview(trimmed, line));
  }

  if (courses.length === 0) {
    throw previewCourseUrlErrorMessage({ type: "empty" });
  }

  return courses;
}

function parseLinkedInCourseUrlForPreview(value: string, line: number): ParsedCourse {
  const withProtocol = value.startsWith("http://") || value.startsWith("https://") ? value : `https://${value}`;
  let url: URL;
  try {
    url = new URL(withProtocol);
  } catch {
    throw previewCourseUrlErrorMessage({ type: "invalidUrl", line });
  }

  const host = url.hostname.toLowerCase();
  const isLinkedIn = host === "linkedin.com" || host.endsWith(".linkedin.com");
  if (!isLinkedIn) {
    throw previewCourseUrlErrorMessage({ type: "notLinkedInLearning", line });
  }

  const segments = url.pathname.split("/").filter(Boolean);
  if (segments[0] !== "learning") {
    throw previewCourseUrlErrorMessage({ type: "notLinkedInLearning", line });
  }

  const slug = segments[1]?.trim();
  if (!slug) {
    throw previewCourseUrlErrorMessage({ type: "missingSlug", line });
  }

  return {
    original: value,
    normalized_url: `https://www.linkedin.com/learning/${slug}`,
    slug,
    quiz_urls: extractQuizUrlsForPreview(url, slug),
    assessment_urns: extractAssessmentUrnsForPreview(url)
  };
}

function extractQuizUrlsForPreview(url: URL, slug: string): string[] {
  const segments = url.pathname.split("/").filter(Boolean);
  const quizIndex = segments.findIndex((segment) => segment.toLowerCase() === "quiz");
  const assessment = quizIndex >= 0 ? segments[quizIndex + 1] : undefined;
  if (!assessment) return [];

  const normalized = new URL(`https://www.linkedin.com/learning/${slug}/quiz/${assessment}`);
  for (const key of ["resume", "u"]) {
    const value = url.searchParams.get(key);
    if (value) normalized.searchParams.set(key, value);
  }
  return [normalized.toString()];
}

function extractAssessmentUrnsForPreview(url: URL): string[] {
  const segments = url.pathname.split("/").filter(Boolean);
  return segments
    .flatMap((segment, index) => (segment.toLowerCase() === "quiz" ? [segments[index + 1]] : []))
    .filter((segment): segment is string => Boolean(segment))
    .filter((segment) => segment.startsWith("urn:li:learningApiAssessment:") || segment.startsWith("urn%3Ali%3AlearningApiAssessment%3A"));
}

function previewCourseUrlErrorMessage(error: PreviewCourseUrlError) {
  if (error.type === "empty") {
    return new Error("no LinkedIn Learning course URLs were provided");
  }
  if (error.type === "notLinkedInLearning") {
    return new Error(`line ${error.line}: expected a linkedin.com/learning course URL`);
  }
  if (error.type === "missingSlug") {
    return new Error(`line ${error.line}: missing course slug`);
  }
  return new Error(`line ${error.line}: could not parse URL`);
}

const previewJobsStorageKey = "linkvault.preview.jobs";
const previewEventsStorageKey = "linkvault.preview.events";
const previewSavedTokenStorageKey = "linkvault.preview.saved-token";
const previewPreferencesStorageKey = "linkvault.preview.preferences";

function getPreviewScenario() {
  if (typeof window === "undefined") return "";
  return new URLSearchParams(window.location.search).get("preview") ?? "";
}

function startDownloadJobsForPreview(request: StartDownloadRequest): StartDownloadResponse {
  const parsed = parseLinkedInCourseUrlsForPreview(request.courseUrls);
  const timestamp = Math.floor(Date.now() / 1000);
  const jobs = parsed.map((course, index) => ({
    id: `preview-job-${index + 1}-${course.slug}`,
    course_slug: course.slug,
    source_url: course.normalized_url,
    status: "queued",
    thumbnail_url: previewThumbnailForSlug(course.slug),
    selected_quality: request.selectedQuality,
    output_dir: request.outputDir,
    updated_at: timestamp,
    artifact_counts: emptyArtifactCounts()
  }));

  writePreviewState(jobs, []);
  return { jobs };
}

function retryFailedDownloadJobForPreview(jobId: string): BootstrapState {
  const timestamp = Math.floor(Date.now() / 1000);
  const jobs = readPreviewJobs();
  const job = jobs.find((candidate) => candidate.id === jobId);
  if (!job) {
    throw new Error("Retry job was not found.");
  }
  if (job.status !== "failed") {
    throw new Error("Only failed jobs can be retried.");
  }

  const retriedJobs = jobs.map((candidate) =>
    candidate.id === jobId
      ? {
          ...candidate,
          status: "queued",
          updated_at: timestamp,
          artifact_counts: emptyArtifactCounts()
        }
      : candidate
  );
  const retryEvent: PersistedJobEvent = {
    id: timestamp,
    job_id: jobId,
    event_type: "job.retry",
    message: "Retry requested; job returned to the queue.",
    created_at: timestamp
  };
  const events = [retryEvent, ...readPreviewEvents()];
  writePreviewState(retriedJobs, events);

  return {
    default_resolution: "P1080",
    browser_sources: ["Chrome", "Edge", "Firefox"],
    stores_plaintext_tokens_in_sqlite: false,
    has_saved_token: hasPreviewSavedToken(),
    saved_download_preferences: readPreviewPreferences(),
    persisted_jobs: retriedJobs,
    recent_events: events
  };
}

async function processNextQueuedDownloadForPreview(): Promise<ProcessQueuedDownloadResponse> {
  const jobs = readPreviewJobs();
  const scenario = getPreviewScenario();
  if (scenario === "live-polling-progress") {
    return processLivePollingProgressForPreview(jobs);
  }

  if (scenario === "metadata-shape-drift") {
    const timestamp = Math.floor(Date.now() / 1000);
    const unsafeRawMetadataBody = "{\"unexpected\":\"unsafe raw body\",\"secret\":\"do-not-render\"}";
    void unsafeRawMetadataBody;

    if (jobs[0]) {
      const failedJob = {
        ...jobs[0],
        status: "failed",
        updated_at: timestamp,
        artifact_counts: emptyArtifactCounts()
      };
      writePreviewState([failedJob, ...jobs.slice(1)], [
        {
          id: 1,
          job_id: failedJob.id,
          event_type: "job.failed",
          message: "Course metadata fetch or artifact planning failed.",
          created_at: timestamp
        }
      ]);
    }

    throw new Error("LinkedIn course metadata shape changed");
  }

  if (scenario === "exercise-404") {
    const timestamp = Math.floor(Date.now() / 1000);
    const unsafeSignedExerciseUrl = "https://cdn.linkedin.example/exercise.zip?signature=do-not-render-signed-url";
    void unsafeSignedExerciseUrl;

    if (jobs[0]) {
      const completedJob = {
        ...jobs[0],
        status: "completed",
        updated_at: timestamp,
        artifact_counts: {
          total: 3,
          completed: 2,
          failed: 1,
          cancelled: 0,
          active: 0,
          pending: 0,
          skipped: 0,
          video_total: 1,
          video_completed: 1,
          subtitle_total: 1,
          subtitle_completed: 1,
          exercise_total: 1,
          exercise_completed: 0
        }
      };
      writePreviewState([completedJob, ...jobs.slice(1)], [
        {
          id: 1,
          job_id: completedJob.id,
          event_type: "artifact.failed",
          message: "Exercise artifact returned 404 and was skipped.",
          created_at: timestamp
        },
        {
          id: 2,
          job_id: completedJob.id,
          event_type: "artifact.completed",
          message: "Video artifact completed after optional exercise failure.",
          created_at: timestamp - 1
        },
        {
          id: 3,
          job_id: completedJob.id,
          event_type: "artifact.completed",
          message: "Subtitle artifact completed after optional exercise failure.",
          created_at: timestamp - 2
        }
      ]);
    }

    return {
      processed: jobs.length > 0,
      completed_artifacts: jobs.length > 0 ? 2 : 0,
      failed_artifacts: jobs.length > 0 ? 1 : 0,
      cancelled_artifacts: 0
    };
  }

  if (scenario === "multi-course-progress") {
    const timestamp = Math.floor(Date.now() / 1000);
    const unsafeQueueSecret = "do-not-render-queue-secret";
    void unsafeQueueSecret;

    const activeJob = jobs[0]
      ? {
          ...jobs[0],
          status: "active",
          updated_at: timestamp,
          artifact_counts: {
            total: 6,
            completed: 3,
            failed: 0,
            cancelled: 0,
            active: 1,
            pending: 2,
            skipped: 0,
            video_total: 3,
            video_completed: 2,
            subtitle_total: 2,
            subtitle_completed: 1,
            exercise_total: 1,
            exercise_completed: 0
          }
        }
      : null;
    const queuedJob = jobs[1]
      ? {
          ...jobs[1],
          status: "queued",
          updated_at: timestamp - 1,
          artifact_counts: {
            total: 4,
            completed: 0,
            failed: 0,
            cancelled: 0,
            active: 0,
            pending: 4,
            skipped: 0,
            video_total: 2,
            video_completed: 0,
            subtitle_total: 1,
            subtitle_completed: 0,
            exercise_total: 1,
            exercise_completed: 0
          }
        }
      : null;

    const nextJobs = [activeJob, queuedJob, ...jobs.slice(2)].filter((job): job is QueuedDownloadJob => job !== null);
    if (nextJobs.length > 0) {
      writePreviewState(nextJobs, [
        {
          id: 1,
          job_id: nextJobs[0].id,
          event_type: "job.active",
          message: "Started first queued course before continuing to the next course.",
          created_at: timestamp
        },
        {
          id: 2,
          job_id: nextJobs[0].id,
          event_type: "artifact.completed",
          message: "First course video and subtitle artifacts are progressing.",
          created_at: timestamp - 1
        }
      ]);
    }

    return {
      processed: nextJobs.length > 0,
      completed_artifacts: activeJob ? 3 : 0,
      failed_artifacts: 0,
      cancelled_artifacts: 0
    };
  }

  if (scenario === "failed-course-lifecycle") {
    const timestamp = Math.floor(Date.now() / 1000);
    const unsafeFailureBody = "{\"error\":\"do-not-render-failed-course-body\",\"li_at\":\"do-not-render-failed-course-token\"}";
    void unsafeFailureBody;

    const failedJob = jobs[0]
      ? {
          ...jobs[0],
          status: "failed",
          updated_at: timestamp,
          artifact_counts: emptyArtifactCounts()
        }
      : null;
    const queuedJob = jobs[1]
      ? {
          ...jobs[1],
          status: "queued",
          updated_at: timestamp - 1,
          artifact_counts: {
            total: 4,
            completed: 0,
            failed: 0,
            cancelled: 0,
            active: 0,
            pending: 4,
            skipped: 0,
            video_total: 2,
            video_completed: 0,
            subtitle_total: 1,
            subtitle_completed: 0,
            exercise_total: 1,
            exercise_completed: 0
          }
        }
      : null;

    const nextJobs = [failedJob, queuedJob, ...jobs.slice(2)].filter((job): job is QueuedDownloadJob => job !== null);
    if (nextJobs.length > 0) {
      writePreviewState(nextJobs, [
        {
          id: 1,
          job_id: nextJobs[0].id,
          event_type: "job.failed",
          message: "First queued course failed before artifact planning; remaining courses stay queued.",
          created_at: timestamp
        }
      ]);
    }

    return {
      processed: nextJobs.length > 0,
      completed_artifacts: 0,
      failed_artifacts: failedJob ? 1 : 0,
      cancelled_artifacts: 0
    };
  }

  if (scenario === "repetitive-artifact-failures") {
    const timestamp = Math.floor(Date.now() / 1000);
    const unsafeFailureUrl = "https://cdn.linkedin.example/exercises.zip?signature=do-not-render-repeated-failure-url";
    void unsafeFailureUrl;

    if (jobs[0]) {
      const completedWithIssuesJob = {
        ...jobs[0],
        status: "completed",
        updated_at: timestamp,
        artifact_counts: {
          total: 8,
          completed: 2,
          failed: 6,
          cancelled: 0,
          active: 0,
          pending: 0,
          skipped: 0,
          video_total: 1,
          video_completed: 1,
          subtitle_total: 1,
          subtitle_completed: 1,
          exercise_total: 6,
          exercise_completed: 0
        }
      };
      writePreviewState([completedWithIssuesJob, ...jobs.slice(1)], [
        {
          id: 1,
          job_id: completedWithIssuesJob.id,
          event_type: "artifact.failed",
          message: "6 exercise artifacts failed; details are coalesced in activity.",
          created_at: timestamp
        },
        {
          id: 2,
          job_id: completedWithIssuesJob.id,
          event_type: "artifact.completed",
          message: "Video and subtitle artifacts completed despite repeated exercise failures.",
          created_at: timestamp - 1
        }
      ]);
    }

    return {
      processed: jobs.length > 0,
      completed_artifacts: jobs.length > 0 ? 2 : 0,
      failed_artifacts: jobs.length > 0 ? 6 : 0,
      cancelled_artifacts: 0
    };
  }

  return {
    processed: jobs.length > 0,
    completed_artifacts: 0,
    failed_artifacts: 0,
    cancelled_artifacts: 0
  };
}

async function processLivePollingProgressForPreview(jobs: QueuedDownloadJob[]): Promise<ProcessQueuedDownloadResponse> {
  const timestamp = Math.floor(Date.now() / 1000);
  const unsafeStreamingToken = "do-not-render-live-polling-token";
  void unsafeStreamingToken;

  if (!jobs[0]) {
    return {
      processed: false,
      completed_artifacts: 0,
      failed_artifacts: 0,
      cancelled_artifacts: 0
    };
  }

  const activeJob = {
    ...jobs[0],
    status: "active",
    updated_at: timestamp,
    artifact_counts: {
      total: 6,
      completed: 1,
      failed: 0,
      cancelled: 0,
      active: 1,
      pending: 4,
      skipped: 0,
      video_total: 3,
      video_completed: 1,
      subtitle_total: 2,
      subtitle_completed: 0,
      exercise_total: 1,
      exercise_completed: 0
    }
  };
  const queuedJob = jobs[1]
    ? {
        ...jobs[1],
        status: "queued",
        updated_at: timestamp,
        artifact_counts: {
          total: 3,
          completed: 0,
          failed: 0,
          cancelled: 0,
          active: 0,
          pending: 3,
          skipped: 0,
          video_total: 1,
          video_completed: 0,
          subtitle_total: 1,
          subtitle_completed: 0,
          exercise_total: 1,
          exercise_completed: 0
        }
      }
    : null;

  writePreviewState([activeJob, queuedJob, ...jobs.slice(2)].filter((job): job is QueuedDownloadJob => job !== null), [
    {
      id: 1,
      job_id: activeJob.id,
      event_type: "job.active",
      message: "Live polling course started.",
      created_at: timestamp
    },
    {
      id: 2,
      job_id: activeJob.id,
      event_type: "artifact.active",
      message: "Live polling course video started.",
      created_at: timestamp
    }
  ]);

  await sleep(900);

  const updatedActiveJob = {
    ...activeJob,
    updated_at: timestamp + 1,
    artifact_counts: {
      ...activeJob.artifact_counts,
      completed: 3,
      active: 1,
      pending: 2,
      video_completed: 2,
      subtitle_completed: 1
    }
  };
  writePreviewState([updatedActiveJob, queuedJob, ...jobs.slice(2)].filter((job): job is QueuedDownloadJob => job !== null), [
    {
      id: 3,
      job_id: activeJob.id,
      event_type: "artifact.completed",
      message: "Live polling course video completed.",
      created_at: timestamp + 1
    },
    {
      id: 2,
      job_id: activeJob.id,
      event_type: "artifact.active",
      message: "Live polling course subtitles started.",
      created_at: timestamp
    }
  ]);

  await sleep(900);

  const completedJob = {
    ...updatedActiveJob,
    status: "completed",
    updated_at: timestamp + 2,
    artifact_counts: {
      ...updatedActiveJob.artifact_counts,
      completed: 6,
      active: 0,
      pending: 0,
      video_completed: 3,
      subtitle_completed: 2,
      exercise_completed: 1
    }
  };
  writePreviewState([completedJob, queuedJob, ...jobs.slice(2)].filter((job): job is QueuedDownloadJob => job !== null), [
    {
      id: 4,
      job_id: activeJob.id,
      event_type: "artifact.extracted",
      message: "Live polling exercise archive extracted.",
      created_at: timestamp + 2
    },
    {
      id: 3,
      job_id: activeJob.id,
      event_type: "artifact.completed",
      message: "Live polling course finished.",
      created_at: timestamp + 1
    }
  ]);

  return {
    processed: true,
    completed_artifacts: 6,
    failed_artifacts: 0,
    cancelled_artifacts: 0
  };
}

function emptyArtifactCounts(): ArtifactProgressCounts {
  return {
    total: 0,
    completed: 0,
    failed: 0,
    cancelled: 0,
    active: 0,
    pending: 0,
    skipped: 0,
    video_total: 0,
    video_completed: 0,
    subtitle_total: 0,
    subtitle_completed: 0,
    quiz_total: 0,
    quiz_completed: 0,
    exercise_total: 0,
    exercise_completed: 0
  };
}

function readPreviewJobs(): QueuedDownloadJob[] {
  if (typeof window === "undefined") return [];
  return parseStoredPreviewValue<QueuedDownloadJob[]>(window.sessionStorage.getItem(previewJobsStorageKey), []);
}

function readPreviewEvents(): PersistedJobEvent[] {
  if (typeof window === "undefined") return [];
  return parseStoredPreviewValue<PersistedJobEvent[]>(window.sessionStorage.getItem(previewEventsStorageKey), []);
}

function writePreviewState(jobs: QueuedDownloadJob[], events: PersistedJobEvent[]) {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(previewJobsStorageKey, JSON.stringify(jobs));
  window.sessionStorage.setItem(previewEventsStorageKey, JSON.stringify(events));
}

function readPreviewPreferences(): SavedDownloadPreferences | null {
  if (typeof window === "undefined") return null;
  return parseStoredPreviewValue<SavedDownloadPreferences | null>(window.sessionStorage.getItem(previewPreferencesStorageKey), null);
}

function writePreviewPreferences(preferences: SavedDownloadPreferences) {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(previewPreferencesStorageKey, JSON.stringify(preferences));
}

function hasPreviewSavedToken() {
  if (typeof window === "undefined") return false;
  return window.sessionStorage.getItem(previewSavedTokenStorageKey) === "true";
}

function setPreviewSavedToken(saved: boolean) {
  if (typeof window === "undefined") return;
  if (saved) {
    window.sessionStorage.setItem(previewSavedTokenStorageKey, "true");
  } else {
    window.sessionStorage.removeItem(previewSavedTokenStorageKey);
  }
}

function parseStoredPreviewValue<T>(value: string | null, fallback: T): T {
  if (!value) return fallback;
  try {
    return JSON.parse(value) as T;
  } catch {
    return fallback;
  }
}

function getBrowserPreviewState(): { jobs: QueuedDownloadJob[]; events: PersistedJobEvent[] } | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const preview = params.get("preview");
  if (!preview) return null;
  return { jobs: readPreviewJobs(), events: readPreviewEvents() };
}

function previewThumbnailForSlug(slug: string) {
  const title = courseDisplayNameFromSlug(slug);
  const initials = courseInitials(title);
  const color = hashColorForSlug(slug);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180"><rect width="320" height="180" fill="${color.background}"/><rect x="16" y="16" width="288" height="148" rx="14" fill="${color.panel}" opacity=".9"/><text x="160" y="104" fill="#ebe7de" font-family="Inter,Arial,sans-serif" font-size="48" font-weight="700" text-anchor="middle">${initials}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

function hashColorForSlug(slug: string) {
  const palettes = [
    { background: "#223843", panel: "#1a4f63" },
    { background: "#2c3224", panel: "#44613d" },
    { background: "#332b3c", panel: "#5c426d" },
    { background: "#3a3026", panel: "#6a523c" }
  ];
  const index = [...slug].reduce((sum, char) => sum + char.charCodeAt(0), 0) % palettes.length;
  return palettes[index];
}
