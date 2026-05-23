import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  Activity,
  Captions,
  CheckCircle2,
  ChevronDown,
  CircleHelp,
  Folder,
  History,
  Import,
  Play,
  Settings,
  SunMedium,
  Trash2,
  Video,
  Wand2,
  X,
  XCircle
} from "lucide-react";
import { IconBrandLinkedin, IconMovie, IconTool } from "@tabler/icons-react";
import { Button, Checkbox, Dialog, Field, IconButton, Input, Panel, Popover, Progress, Select, Textarea, Tooltip, guardedToast } from "./components/primitives";

type ParsedCourse = {
  original: string;
  normalized_url: string;
  slug: string;
};

type ValidatedLinkedInSession = {
  csrf_token: string;
  enterprise_profile_hash: string | null;
  request_headers: [string, string][];
};

type QueuedDownloadJob = {
  id: string;
  course_slug: string;
  source_url: string;
  status: string;
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
};

type BootstrapState = {
  default_resolution: string;
  browser_sources: string[];
  stores_plaintext_tokens_in_sqlite: boolean;
  saved_download_preferences: SavedDownloadPreferences | null;
  persisted_jobs: QueuedDownloadJob[];
  recent_events: PersistedJobEvent[];
};

type PreviewCourseUrlError =
  | { type: "empty" }
  | { type: "notLinkedInLearning"; line: number }
  | { type: "missingSlug"; line: number }
  | { type: "invalidUrl"; line: number };

export default function App() {
  const [courseUrls, setCourseUrls] = useState("");
  const [folder, setFolder] = useState("C:/Users/howard/Downloads/LinkedIn Courses");
  const [token, setToken] = useState("");
  const [resolution, setResolution] = useState("1080");
  const [browserSource, setBrowserSource] = useState("Chrome");
  const [delaySeconds, setDelaySeconds] = useState(0);
  const [downloadVideos, setDownloadVideos] = useState(true);
  const [downloadExercises, setDownloadExercises] = useState(true);
  const [downloadSubtitles, setDownloadSubtitles] = useState(true);
  const [parsedCourses, setParsedCourses] = useState<ParsedCourse[]>([]);
  const [validatedSession, setValidatedSession] = useState<ValidatedLinkedInSession | null>(null);
  const [isImportingToken, setIsImportingToken] = useState(false);
  const [isValidatingToken, setIsValidatingToken] = useState(false);
  const [isProcessingDownload, setIsProcessingDownload] = useState(false);
  const [isCancellingDownload, setIsCancellingDownload] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isHelpOpen, setIsHelpOpen] = useState(false);
  const [queuedJobs, setQueuedJobs] = useState<QueuedDownloadJob[]>([]);
  const [persistedEvents, setPersistedEvents] = useState<PersistedJobEvent[]>([]);
  const [processingSummary, setProcessingSummary] = useState<ProcessQueuedDownloadResponse | null>(null);

  useEffect(() => {
    refreshBootstrapState();
  }, []);

  async function refreshBootstrapState() {
    try {
      const state = await invoke<BootstrapState>("bootstrap_state");
      const preferences = state.saved_download_preferences;
      if (preferences) {
        setFolder(preferences.outputDir);
        setResolution(preferences.selectedQuality);
        setDelaySeconds(preferences.delaySeconds);
        setBrowserSource(preferences.browserSource);
        setDownloadVideos(preferences.downloadVideos);
        setDownloadExercises(preferences.downloadExercises);
        setDownloadSubtitles(preferences.downloadSubtitles);
      } else if (state.default_resolution) {
        setResolution(String(state.default_resolution).replace("P", ""));
      }

      if (state.persisted_jobs.length > 0) {
        setQueuedJobs(state.persisted_jobs);
      } else {
        setQueuedJobs([]);
      }
      setPersistedEvents(state.recent_events ?? []);
    } catch {
      // Browser-only Vite previews do not expose Tauri commands.
      const previewState = getBrowserPreviewState();
      if (previewState) {
        setQueuedJobs(previewState.jobs);
        setPersistedEvents(previewState.events);
      }
    }
  }

  const canStart = useMemo(
    () => courseUrls.trim().length > 0 && (token.trim().length > 0 || validatedSession !== null) && !isProcessingDownload,
    [courseUrls, token, validatedSession, isProcessingDownload]
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

  const activeQueueJobs = queuedJobs.filter((job) => !isTerminalJob(job.status));
  const terminalJobs = queuedJobs.filter((job) => isTerminalJob(job.status));
  const liveProgressJob = activeQueueJobs.find((job) => job.status === "active") ?? activeQueueJobs[0] ?? null;
  const persistedActivityEvents = persistedEvents.map((event) => [
    formatEventTime(event.created_at),
    event.message,
    eventTone(event.event_type)
  ]);

  const queueSummary = queuedJobs.length > 0
    ? [
        queueCounts.active ? `${queueCounts.active} active` : null,
        queueCounts.queued ? `${queueCounts.queued} queued` : null,
        queueCounts.completed ? `${queueCounts.completed} completed` : null,
        queueCounts.failed ? `${queueCounts.failed} failed` : null,
        queueCounts.cancelled ? `${queueCounts.cancelled} cancelled` : null
      ].filter(Boolean).join(" - ")
    : "No persisted jobs";

  const activityEvents = processingSummary?.processed
    ? [
        [
          "Now",
          `Processed queued job: ${processingSummary.completed_artifacts} completed, ${processingSummary.failed_artifacts} failed, ${processingSummary.cancelled_artifacts} cancelled`,
          processingSummary.failed_artifacts > 0 ? "danger" : "success"
        ],
        ...persistedActivityEvents
      ]
    : persistedActivityEvents;

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
    let session = validatedSession;
    if (token.trim()) {
      setIsValidatingToken(true);
      try {
        session = await validateLinkedInToken(token);
        setValidatedSession(session);
      } catch (error) {
        toast.error("Token validation failed", { description: String(error) });
        setIsValidatingToken(false);
        return;
      }
      setIsValidatingToken(false);
    } else if (!session) {
      toast.warning("LinkedIn token required", { description: "Paste li_at or import a browser token before starting." });
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
        downloadSubtitles
      });
      setQueuedJobs(response.jobs);
      toast.success("Download queued", {
        description: `${response.jobs.length} LinkedIn course${response.jobs.length === 1 ? "" : "s"} persisted to the local queue.`
      });

      const processResponse = token.trim()
        ? await processNextQueuedDownloadWithToken(token)
        : await processNextQueuedDownloadFromBrowserSource(browserSource);

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

  async function importToken() {
    setIsImportingToken(true);
    try {
      const session = await invoke<ValidatedLinkedInSession>("validate_browser_token_source", { source: browserSource });
      setValidatedSession(session);
      toast.success("Browser token validated", {
        description: `${browserSource} provided a usable LinkedIn Learning session.`
      });
    } catch (error) {
      toast.error("Browser token import failed", {
        description: String(error)
      });
    } finally {
      setIsImportingToken(false);
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
    <div className="lv-shell">
      <aside className="lv-sidebar" aria-label="Primary navigation">
        <div className="grid gap-0.5 border-b border-sidebar-border px-3 pb-7 pt-4">
          <div className="flex h-7 items-center gap-2">
            <span className="h-2.5 w-2.5 rounded-full bg-window-red" />
            <span className="h-2.5 w-2.5 rounded-full bg-window-yellow" />
            <span className="h-2.5 w-2.5 rounded-full bg-window-green" />
          </div>
          <h1 className="mt-5 text-xl font-semibold tracking-normal text-sidebar-foreground">LinkVault</h1>
          <p className="text-xs text-sidebar-muted">Course and video archive</p>
        </div>

        <nav className="grid flex-1 content-start gap-1 px-3 py-3 text-sm">
          <button className="lv-nav-row active" type="button">
            <IconBrandLinkedin aria-hidden="true" size={18} />
            <span>LinkedIn Courses</span>
          </button>
          <button className="lv-nav-row disabled" type="button" aria-disabled="true" title="Unavailable in the LinkedIn Learning MVP">
            <IconMovie aria-hidden="true" size={18} />
            <span>Generic Video</span>
          </button>
          <button className="lv-nav-row" type="button">
            <IconTool aria-hidden="true" size={18} />
            <span>Tools</span>
          </button>
          <button className="lv-nav-row" type="button">
            <History aria-hidden="true" />
            <span>History</span>
          </button>
          <div className="mt-7 flex items-center justify-between border-t border-sidebar-border pt-6 text-xs text-sidebar-muted">
            <span>LinkedIn Scraper</span>
            <span className="rounded-full border border-sidebar-border px-2 py-0.5 text-[11px]">Out of scope</span>
          </div>
          <button className="lv-nav-row mt-5" type="button">
            <Settings aria-hidden="true" />
            <span>Settings</span>
          </button>
        </nav>

        <div className="flex items-center justify-between px-6 py-4 text-xs text-sidebar-muted">
          <span>v0.1.0</span>
          <div className="flex items-center gap-2">
            <SunMedium aria-hidden="true" className="h-4 w-4" />
            <Popover
              label="LinkVault help"
              open={isHelpOpen}
              onOpenChange={setIsHelpOpen}
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
                Generic Video and LinkedIn Scraper are visible for context only. Course downloads use manual or browser-imported LinkedIn sessions.
              </p>
            </Popover>
          </div>
        </div>
      </aside>

      <main className="lv-main">
        <header className="lv-header">
          <div className="flex min-w-0 items-center gap-3">
            <span className="grid h-6 w-6 place-items-center rounded-md border border-primary/50 bg-primary/10 text-primary">
              <Play aria-hidden="true" className="h-3.5 w-3.5" />
            </span>
            <h2 className="truncate text-xl font-semibold text-foreground">LinkedIn Courses</h2>
          </div>
          <div className="ml-auto flex items-center gap-3">
            <span className="inline-flex h-8 items-center gap-2 rounded-md border border-success/25 bg-success/10 px-3 text-xs text-foreground">
              <span className="h-2 w-2 rounded-full bg-success" />
              Downloader online
            </span>
            <Tooltip label="Open settings">
              <IconButton aria-label="Open settings" onClick={() => setIsSettingsOpen(true)}>
                <Settings aria-hidden="true" className="h-4 w-4" />
              </IconButton>
            </Tooltip>
          </div>
        </header>

        <div className="lv-content">
          <div className="grid min-w-0 gap-3">
            <Panel className="p-5">
              <div className="mb-5 flex items-start gap-3">
                <span className="mt-0.5 grid h-5 w-5 place-items-center rounded border border-primary/50 text-primary">
                  <Video aria-hidden="true" className="h-3.5 w-3.5" />
                </span>
                <div className="min-w-0">
                  <h3 className="text-base font-semibold text-foreground">Course Setup</h3>
                  <p className="mt-1 text-xs text-muted">Configure your download and start archiving LinkedIn Learning courses.</p>
                </div>
              </div>

              <div className="grid gap-3">
                <Field label="Course URLs">
                  <div className="grid grid-cols-[36px_minmax(0,1fr)] overflow-hidden rounded-md border border-input bg-field">
                    <div className="border-r border-input px-3 py-2 text-right text-xs leading-5 text-muted">1</div>
                    <Textarea
                      value={courseUrls}
                      onChange={(event) => {
                        setCourseUrls(event.target.value);
                        setParsedCourses([]);
                      }}
                      onBlur={validateUrls}
                      placeholder="One course URL per line"
                      spellCheck={false}
                      className="min-h-20 border-0 bg-transparent focus:ring-0"
                      aria-label="Course URLs"
                    />
                  </div>
                </Field>

                <Field label="Download folder">
                  <div className="field-action-grid">
                    <Input value={folder} onChange={(event) => setFolder(event.target.value)} aria-label="Download folder" />
                    <Button type="button" onClick={browseDownloadFolder}>
                      <Folder aria-hidden="true" className="h-4 w-4" />
                      Browse
                    </Button>
                  </div>
                </Field>

                <Field label="Token cookie">
                  <div className="field-action-grid">
                    <Input
                      value={token}
                      onChange={(event) => {
                        setToken(event.target.value);
                        setValidatedSession(null);
                      }}
                      placeholder="Paste your LinkedIn li_at cookie value"
                      type="password"
                      aria-label="LinkedIn li_at token"
                    />
                    <Button type="button" onClick={() => {
                      setToken("");
                      setValidatedSession(null);
                    }}>
                      <Trash2 aria-hidden="true" className="h-4 w-4" />
                      Clear
                    </Button>
                  </div>
                </Field>

                <div className="grid gap-3 md:grid-cols-2">
                  <Field label="Video resolution">
                    <Select value={resolution} onChange={(event) => setResolution(event.target.value)} aria-label="Video resolution">
                      <option value="1080">1080p (Best available)</option>
                      <option value="720">720p</option>
                      <option value="540">540p</option>
                      <option value="360">360p</option>
                    </Select>
                  </Field>
                  <Field label="Browser token source">
                    <Select
                      value={browserSource}
                      onChange={(event) => {
                        setBrowserSource(event.target.value);
                        setValidatedSession(null);
                      }}
                      aria-label="Browser token source"
                    >
                      <option>Chrome</option>
                      <option>Edge</option>
                      <option>Firefox</option>
                    </Select>
                  </Field>
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <Field label="Delay seconds">
                    <Input
                      value={delaySeconds}
                      type="number"
                      min={0}
                      onChange={(event) => setDelaySeconds(Number(event.target.value))}
                      aria-label="Delay seconds"
                    />
                  </Field>
                  <div className="grid content-end gap-1.5 pb-0.5">
                    <Checkbox checked={downloadVideos} onChange={(event) => setDownloadVideos(event.target.checked)} label="Download videos" />
                    <Checkbox checked={downloadExercises} onChange={(event) => setDownloadExercises(event.target.checked)} label="Download exercise files" />
                    <Checkbox checked={downloadSubtitles} onChange={(event) => setDownloadSubtitles(event.target.checked)} label="Download subtitles" />
                  </div>
                </div>
              </div>

              <div className="mt-5 flex flex-wrap justify-end gap-2">
                <Button type="button" onClick={importToken} disabled={isImportingToken || isValidatingToken || isProcessingDownload}>
                  <Import aria-hidden="true" className="h-4 w-4" />
                  {isImportingToken ? "Importing" : "Import Token"}
                </Button>
                <Button type="button" variant="primary" onClick={startDownload} disabled={!canStart || isImportingToken || isValidatingToken || isProcessingDownload}>
                  <Play aria-hidden="true" className="h-4 w-4" />
                  {isValidatingToken ? "Validating" : isProcessingDownload ? "Processing" : "Start Download"}
                </Button>
                <Button type="button" variant="outline" onClick={cancelDownload} disabled={!isProcessingDownload || isCancellingDownload}>
                  <X aria-hidden="true" className="h-4 w-4" />
                  {isCancellingDownload ? "Cancelling" : "Cancel"}
                </Button>
              </div>
            </Panel>

            <Panel className="overflow-hidden">
              <div className="flex h-12 items-center justify-between border-b border-border px-4">
                <h3 className="text-sm font-semibold">Download Queue</h3>
                {queuedJobs.length > 0 ? <span className="text-xs text-muted">{queueSummary}</span> : parsedCourses.length > 0 ? <span className="text-xs text-muted">{parsedCourses.length} validated</span> : null}
              </div>
              <div className="grid gap-2 p-3">
                {activeQueueJobs.length > 0 ? (
                  activeQueueJobs.map((job) => <PersistedQueueRow key={job.id} job={job} />)
                ) : queuedJobs.length > 0 ? (
                  <EmptyPanelText title="No active queue" description="Completed, failed, and cancelled jobs are shown in History." />
                ) : parsedCourses.length > 0 ? (
                  <ValidatedCoursePreview courses={parsedCourses} />
                ) : (
                  <EmptyPanelText title="No persisted jobs" description="Validated LinkedIn Learning courses will appear here after Start Download persists them." />
                )}
              </div>
              <div className="flex min-h-12 items-center justify-between border-t border-border px-4 text-sm text-muted">
                <span>{queueSummary}</span>
                <Button size="sm">
                  <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                  Clear completed
                </Button>
              </div>
            </Panel>
          </div>

          <Panel className="lv-activity overflow-hidden">
            <div className="flex h-14 items-center gap-3 border-b border-border px-4">
              <Activity aria-hidden="true" className="h-4 w-4 text-primary" />
              <h3 className="text-base font-semibold">Activity</h3>
            </div>
            <div className="grid gap-3 p-4">
              <div className="flex items-center justify-between">
                <h4 className="text-sm font-semibold">Live Progress</h4>
                <Button size="sm">View all</Button>
              </div>
              {liveProgressJob ? (
                <LiveProgressCard job={liveProgressJob} />
              ) : (
                <EmptyPanelText title="No live download" description="The next active or queued course will show SQLite artifact counts here." />
              )}

              <div className="mt-1 flex items-center justify-between">
                <h4 className="text-sm font-semibold">Recent Activity</h4>
                <Button size="sm">Clear</Button>
              </div>
              <ol className="grid gap-0 border-l border-border pl-4">
                {activityEvents.length > 0 ? activityEvents.map(([time, label, tone]) => (
                  <li key={`${time}-${label}`} className="relative grid grid-cols-[64px_minmax(0,1fr)] gap-2 py-2 text-xs">
                    <span className={`absolute -left-[19px] top-3.5 h-1.5 w-1.5 rounded-full ${activityDotClass(tone)}`} />
                    <time className="text-muted">{time}</time>
                    <span className="line-clamp-2 text-muted-strong">{label}</span>
                  </li>
                )) : (
                  <li className="py-3 text-xs text-muted">No persisted activity yet.</li>
                )}
              </ol>

              <div className="mt-2 flex items-center justify-between border-t border-border pt-4">
                <h4 className="text-sm font-semibold">Completed</h4>
                <Button size="sm">View all</Button>
              </div>
              <div className="grid gap-2">
                {terminalJobs.length > 0 ? (
                  terminalJobs.slice(0, 3).map((job) => <HistoryJobRow key={job.id} job={job} />)
                ) : (
                  <EmptyPanelText title="No completed jobs" description="Finished and failed downloads will appear here after processing." />
                )}
              </div>
            </div>
          </Panel>
        </div>
      </main>
    </div>
    <Dialog
      open={isSettingsOpen}
      onOpenChange={setIsSettingsOpen}
      title="LinkVault settings"
      description="Local downloader settings are restored from SQLite without storing plaintext LinkedIn tokens."
    >
      <div className="grid gap-3 text-sm">
        <div className="rounded-md border border-border bg-field p-3">
          <div className="text-xs font-semibold uppercase text-muted">Default output</div>
          <div className="mt-1 truncate text-muted-strong" title={folder}>{folder}</div>
        </div>
        <div className="grid gap-2 rounded-md border border-border bg-field p-3 text-xs text-muted">
          <div className="flex items-center justify-between gap-3">
            <span>Default resolution</span>
            <span className="font-medium text-muted-strong">{resolution}p</span>
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>Browser source</span>
            <span className="font-medium text-muted-strong">{browserSource}</span>
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>Plaintext token storage</span>
            <span className="font-medium text-success">Disabled</span>
          </div>
        </div>
      </div>
    </Dialog>
    </>
  );
}

function isTerminalJob(status: string) {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function formatEventTime(timestamp: number) {
  if (!timestamp) return "--:--";
  return new Date(timestamp * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
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

function ValidatedCoursePreview({ courses }: { courses: ParsedCourse[] }) {
  return (
    <div className="grid gap-2">
      {courses.map((course, index) => (
        <div key={`${course.slug}-${index}`} className="grid grid-cols-[32px_minmax(0,1fr)] items-center gap-3 rounded-lg border border-border bg-field p-3">
          <div className="grid h-8 w-8 place-items-center rounded-md bg-primary/15 text-xs font-semibold text-primary">{index + 1}</div>
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold" title={course.slug}>{courseDisplayNameFromSlug(course.slug)}</div>
            <div className="mt-1 truncate text-xs text-muted" title={course.normalized_url}>{course.normalized_url}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

function LiveProgressCard({ job }: { job: QueuedDownloadJob }) {
  const counts = artifactCounts(job);
  const progress = artifactProgressPercent(counts.completed, counts.total, job.status);
  const title = courseDisplayName(job);

  return (
    <div className="rounded-lg border border-border bg-field p-3">
      <div className="grid grid-cols-[104px_minmax(0,1fr)] gap-3">
        <CourseTile title={title} status={job.status} />
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold" title={title}>{title}</div>
          <div className="mt-1 truncate text-xs text-muted" title={job.source_url}>{job.source_url}</div>
          <div className="mt-4 grid grid-cols-[1fr_auto] items-center gap-3 text-xs">
            <Progress value={progress} />
            <span className="font-medium text-primary">{progress}%</span>
          </div>
          <div className="mt-1 text-xs text-muted">{artifactSummaryText(counts, job.status)}</div>
        </div>
      </div>
      <div className="mt-4 grid gap-2 text-sm text-muted">
        <ProgressLine icon={<Video aria-hidden="true" />} label="Videos" completed={counts.video_completed} total={counts.video_total} />
        <ProgressLine icon={<Captions aria-hidden="true" />} label="Subtitles" completed={counts.subtitle_completed} total={counts.subtitle_total} />
        <ProgressLine icon={<Wand2 aria-hidden="true" />} label="Exercise files" completed={counts.exercise_completed} total={counts.exercise_total} />
      </div>
    </div>
  );
}

function PersistedQueueRow({ job }: { job: QueuedDownloadJob }) {
  const counts = artifactCounts(job);
  const progress = artifactProgressPercent(counts.completed, counts.total, job.status);
  const title = courseDisplayName(job);
  const isCompleted = job.status === "completed";

  return (
    <div className="queue-row queue-row-active">
      <CourseTile title={title} status={job.status} />
      <div className="min-w-0">
        <div className="flex min-w-0 flex-col gap-1 sm:flex-row sm:items-center sm:gap-3">
          <div className="truncate text-sm font-semibold" title={title}>{title}</div>
          <span className="shrink-0 text-xs text-muted">{artifactSummaryText(counts, job.status)}</span>
          <span className={`shrink-0 rounded-full px-2 py-0.5 text-[11px] ${jobStatusBadgeClass(job.status)}`}>{job.status}</span>
        </div>
        <div className="mt-3 grid grid-cols-[104px_minmax(0,1fr)_58px_38px] items-center gap-3 text-xs text-muted">
          <span>Overall progress</span>
          <Progress value={progress} />
          <span>{counts.completed} / {counts.total}</span>
          <span>{progress}%</span>
          <QueueProgressLine label="Videos" completed={counts.video_completed} total={counts.video_total} />
          <QueueProgressLine label="Subtitles" completed={counts.subtitle_completed} total={counts.subtitle_total} />
          <QueueProgressLine label="Exercise files" completed={counts.exercise_completed} total={counts.exercise_total} />
        </div>
      </div>
      <div className={`text-right text-sm font-semibold ${isCompleted ? "text-success" : "text-primary"}`}>{progress}%</div>
      {isCompleted ? (
        <CheckCircle2 aria-hidden="true" className="h-5 w-5 text-success" />
      ) : (
        <IconButton aria-label={`Actions for ${title}`} disabled>
          <ChevronDown aria-hidden="true" className="h-4 w-4" />
        </IconButton>
      )}
    </div>
  );
}

function HistoryJobRow({ job }: { job: QueuedDownloadJob }) {
  const failed = job.status === "failed";
  const cancelled = job.status === "cancelled";
  const Icon = failed || cancelled ? XCircle : CheckCircle2;
  const iconClass = failed ? "text-danger" : cancelled ? "text-muted" : "text-success";
  const statusText = job.status.charAt(0).toUpperCase() + job.status.slice(1);
  const title = courseDisplayName(job);
  const counts = artifactCounts(job);

  return (
    <div className="grid grid-cols-[24px_96px_minmax(0,1fr)] items-center gap-3 rounded-lg border border-border bg-field p-3">
      <Icon aria-hidden="true" className={`h-5 w-5 ${iconClass}`} />
      <CourseTile title={title} status={job.status} compact />
      <div className="min-w-0">
        <div className="truncate text-sm font-semibold" title={title}>{title}</div>
        <div className="mt-1 truncate text-xs text-muted">{statusText} - {formatEventTime(job.updated_at ?? 0)} - {artifactSummaryText(counts, job.status)}</div>
        <div className="mt-1 truncate text-xs text-muted" title={job.output_dir ?? job.source_url}>{job.output_dir ?? job.source_url}</div>
      </div>
    </div>
  );
}

function EmptyPanelText({ title, description }: { title: string; description: string }) {
  return (
    <div className="rounded-lg border border-dashed border-border bg-field/60 p-3">
      <div className="text-sm font-semibold text-muted-strong">{title}</div>
      <div className="mt-1 text-xs leading-5 text-muted">{description}</div>
    </div>
  );
}

function CourseTile({ title, status, compact = false }: { title: string; status: string; compact?: boolean }) {
  const tileClass = status === "failed"
    ? "bg-danger/40"
    : status === "cancelled"
      ? "bg-muted/30"
      : status === "completed"
        ? "bg-success/40"
        : "bg-primary/30";
  return (
    <div
      className={`grid place-items-center rounded-md px-2 text-center font-semibold leading-tight text-white shadow-inner-card ${compact ? "h-12 text-[9px]" : "h-16 text-[11px]"} ${tileClass}`}
      title={title}
    >
      <span className="line-clamp-2 break-words">{title}</span>
    </div>
  );
}

function ProgressLine({ icon, label, completed, total }: { icon: React.ReactNode; label: string; completed: number; total: number }) {
  const percent = artifactProgressPercent(completed, total, total === 0 ? "queued" : "active");
  return (
    <div className="grid grid-cols-[18px_minmax(0,1fr)_58px_38px] items-center gap-2">
      <span className="[&_svg]:h-3.5 [&_svg]:w-3.5">{icon}</span>
      <span className="truncate">{label}</span>
      <span className="text-right">{completed} / {total}</span>
      <span className="text-right">{percent}%</span>
    </div>
  );
}

function QueueProgressLine({ label, completed, total }: { label: string; completed: number; total: number }) {
  const percent = artifactProgressPercent(completed, total, total === 0 ? "queued" : "active");
  return (
    <>
      <span className="truncate">{label}</span>
      <Progress value={percent} />
      <span>{completed} / {total}</span>
      <span>{percent}%</span>
    </>
  );
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

function showProcessedDownloadToast(response: ProcessQueuedDownloadResponse) {
  const description = `${response.completed_artifacts} completed, ${response.failed_artifacts} failed, ${response.cancelled_artifacts} cancelled.`;
  if (response.failed_artifacts > 0 || response.cancelled_artifacts > 0) {
    toast.warning("Queued download processed with issues", { description });
    return;
  }

  toast.success("Queued download processed", { description });
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

async function validateLinkedInToken(token: string) {
  if (isTauriRuntime()) {
    return invoke<ValidatedLinkedInSession>("validate_li_at_token", { token });
  }

  if (!token.trim()) {
    throw new Error("LinkedIn token required");
  }

  return {
    csrf_token: "preview-csrf-token",
    enterprise_profile_hash: null,
    request_headers: [["Csrf-Token", "preview-csrf-token"]]
  } satisfies ValidatedLinkedInSession;
}

async function startDownloadJobs(request: StartDownloadRequest) {
  if (isTauriRuntime()) {
    return invoke<StartDownloadResponse>("start_download_jobs", { request });
  }

  return startDownloadJobsForPreview(request);
}

async function processNextQueuedDownloadWithToken(token: string) {
  if (isTauriRuntime()) {
    return invoke<ProcessQueuedDownloadResponse>("process_next_queued_download_with_li_at", { token });
  }

  return processNextQueuedDownloadForPreview();
}

async function processNextQueuedDownloadFromBrowserSource(source: string) {
  if (isTauriRuntime()) {
    return invoke<ProcessQueuedDownloadResponse>("process_next_queued_download_from_browser_source", { source });
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
    slug
  };
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
    selected_quality: request.selectedQuality,
    output_dir: request.outputDir,
    updated_at: timestamp,
    artifact_counts: emptyArtifactCounts()
  }));

  writePreviewState(jobs, []);
  return { jobs };
}

function processNextQueuedDownloadForPreview(): ProcessQueuedDownloadResponse {
  const jobs = readPreviewJobs();
  const scenario = getPreviewScenario();
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
  if (preview && preview !== "long-labels") {
    return { jobs: readPreviewJobs(), events: readPreviewEvents() };
  }
  if (preview !== "long-labels") return null;
  const now = Math.floor(Date.now() / 1000);
  const longSlug = "advanced-linkedin-learning-course-with-a-very-long-title-for-rendering-queue-progress-history-and-url-wrapping";
  return {
    jobs: [
      {
        id: "preview-active-long-label",
        course_slug: longSlug,
        source_url: `https://www.linkedin.com/learning/${longSlug}?trk=share&u=123456&context=long-render-check`,
        status: "active",
        selected_quality: "1080",
        output_dir: "C:/Users/howard/Downloads/LinkedIn Courses/Advanced LinkedIn Learning Course With A Very Long Title",
        updated_at: now,
        artifact_counts: {
          total: 37,
          completed: 18,
          failed: 1,
          cancelled: 0,
          active: 1,
          pending: 17,
          skipped: 0,
          video_total: 18,
          video_completed: 9,
          subtitle_total: 18,
          subtitle_completed: 8,
          exercise_total: 1,
          exercise_completed: 1
        }
      },
      {
        id: "preview-completed-long-label",
        course_slug: "completed-course-with-long-output-folder-name-and-history-label",
        source_url: "https://www.linkedin.com/learning/completed-course-with-long-output-folder-name-and-history-label",
        status: "completed",
        selected_quality: "720",
        output_dir: "C:/Users/howard/Downloads/LinkedIn Courses/Completed Course With Long Output Folder Name And History Label",
        updated_at: now - 240,
        artifact_counts: {
          total: 12,
          completed: 12,
          failed: 0,
          cancelled: 0,
          active: 0,
          pending: 0,
          skipped: 0,
          video_total: 6,
          video_completed: 6,
          subtitle_total: 5,
          subtitle_completed: 5,
          exercise_total: 1,
          exercise_completed: 1
        }
      }
    ],
    events: [
      {
        id: 1,
        job_id: "preview-active-long-label",
        event_type: "artifact.completed",
        message: "Downloading video with an intentionally long chapter and lesson title that should clamp without pushing the activity panel wider.",
        created_at: now
      },
      {
        id: 2,
        job_id: "preview-active-long-label",
        event_type: "job.active",
        message: `Started LinkedIn Learning course: https://www.linkedin.com/learning/${longSlug}?trk=share&u=123456&context=long-render-check`,
        created_at: now - 60
      }
    ]
  };
}
