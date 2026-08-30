import { type DragEvent as ReactDragEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CalendarClock,
  CircleHelp,
  Download,
  Folder,
  FolderOpen,
  GripVertical,
  LoaderCircle,
  Pause,
  Play,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import { ensureDestination, parseDestination } from "../../lib/destinations";
import { writeNewspaperDestination } from "../../lib/newspaper/preferences";
import { Button, Checkbox, Input, Select, StatusBadge, Switch, Tooltip } from "../primitives";
import { NewspaperLibrary } from "./NewspaperLibrary";
import { readNewspaperOptimizationPreferences, type NewspaperOptimizationRunOptions } from "./newspaper-optimization-preferences";
import type { NewspaperReaderSourceTarget } from "./newspaper-navigation";

type EditionKind = "daily" | "weekly" | "special";
type NewspaperEdition = {
  code: string;
  nameZh: string;
  nameEn: string;
  kind: EditionKind;
  schedule: "daily" | "weekly_sunday" | "ad_hoc";
  sourceUrl: string;
  publicationDate?: string | null;
  discovered: boolean;
};
type NewspaperBatch = {
  id: string;
  status: string;
  destination: string;
  scheduled_at?: number | null;
  delay_seconds: number;
};
type NewspaperSchedule = {
  id: string;
  enabled: boolean;
  cron_time: string;
  destination: string;
  edition_codes: string[];
  date_mode: "single" | "last7_days";
  delay_seconds: number;
  optimization_quality: number;
  last_run_date?: string | null;
  last_error?: string | null;
};
type NewspaperJob = {
  id: string;
  batch_id: string;
  edition_code: string;
  edition_name: string;
  publication_date: string;
  status: string;
  output_dir: string;
  page_count: number;
  completed_count: number;
  failed_count: number;
  retry_at?: number | null;
  retry_count: number;
  warning?: string | null;
  queue_position: number;
  paused: boolean;
  dismissed: boolean;
  created_at: number;
  updated_at: number;
  completed_at?: number | null;
};
type Bootstrap = {
  catalog: NewspaperEdition[];
  batches: NewspaperBatch[];
  jobs: NewspaperJob[];
  schedules: NewspaperSchedule[];
  settings: Record<string, unknown>;
};
type ActivitySnapshot = {
  jobs: NewspaperJob[];
  progress: NewspaperJobProgress[];
  batches: NewspaperBatch[];
  schedules: NewspaperSchedule[];
  hasLiveActivity: boolean;
  optimizationRuntime: OptimizationRuntime;
  revision: number;
};
type NewspaperJobProgress = {
  jobId: string;
  currentStage: string;
  downloadTotal: number;
  downloadCompleted: number;
  downloadFailed: number;
  optimizationTotal: number;
  optimizationCompleted: number;
  optimizationFailed: number;
  optimizationPending: number;
  optimizationRecovered: number;
  activeWorkers: number;
  pagesPerMinute?: number | null;
  etaSeconds?: number | null;
  originalBytes: number;
  optimizedBytes: number;
  bytesSaved: number;
};
type OptimizationRuntime = {
  active: boolean;
  mode: string;
  requestedWorkers: number;
  admittedWorkers: number;
  activeWorkers: number;
  cpuPercent?: number | null;
  availableMemoryBytes?: number | null;
  memorySafe: boolean;
  limitedReason?: string | null;
};
type NewspaperQueueSection = "queue" | "active" | "completed" | "failed";
type CreateBatchResponse = {
  jobs: NewspaperJob[];
  skippedCount: number;
};
const PREF_KEY = "linkvault.newspaper.preferences";
const WEBP_QUALITY_PRESETS: readonly number[] = [92, 86, 74, 55, 45, 35, 25];
const FALLBACK_CATALOG: NewspaperEdition[] = [
  ["NY", "紐約", "New York", "daily"],
  ["LA", "洛杉磯", "Los Angeles", "daily"],
  ["SF", "舊金山", "San Francisco", "daily"],
  ["NJ", "新賓", "New Jersey / Pennsylvania", "daily"],
  ["DC", "大華府", "Washington, D.C.", "daily"],
  ["BO", "波士頓", "Boston", "daily"],
  ["AT", "美東南", "Southeast U.S.", "daily"],
  ["CH", "芝加哥", "Chicago", "daily"],
  ["TX", "德州", "Texas", "daily"],
  ["SE", "西雅圖／夏威夷", "Seattle / Hawaii", "daily"],
  ["NW", "世界周刊（美東）", "Weekly — East", "weekly"],
  ["LW", "世界周刊（美西南）", "Weekly — Southwest", "weekly"],
  ["SW", "世界周刊（美西北）", "Weekly — Northwest", "weekly"]
].map(([code, nameZh, nameEn, kind]) => ({
  code,
  nameZh,
  nameEn,
  kind: kind as EditionKind,
  schedule: kind === "weekly" ? "weekly_sunday" : "daily",
  sourceUrl: `https://ep.worldjournal.com/${code}`,
  discovered: false
}));

function today() {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

function editionKey(edition: NewspaperEdition) {
  return edition.publicationDate ? `${edition.code}@${edition.publicationDate}` : edition.code;
}

export function NewspaperView({
  mode = "download",
  onRequestQueueProcess,
  onOpenClipping,
  onReturnClipping,
  readerTarget,
  onReaderTargetConsumed
}: {
  mode?: "download" | "library";
  onRequestQueueProcess?: (options?: NewspaperOptimizationRunOptions | null) => void | Promise<void>;
  onOpenClipping?: (clippingId: string) => void;
  onReturnClipping?: (clippingId: string) => void;
  readerTarget?: NewspaperReaderSourceTarget | null;
  onReaderTargetConsumed?: (generation: number) => void;
}) {
  const initial = useRef(readPreferences());
  const [catalog, setCatalog] = useState<NewspaperEdition[]>(FALLBACK_CATALOG);
  const [jobs, setJobs] = useState<NewspaperJob[]>([]);
  const [batches, setBatches] = useState<NewspaperBatch[]>([]);
  const [schedules, setSchedules] = useState<NewspaperSchedule[]>([]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(initial.current.selected ?? ["NY"]));
  const [kind, setKind] = useState<"all" | EditionKind>("all");
  const [dateMode, setDateMode] = useState<"single" | "last7_days" | "custom">("single");
  const [startDate, setStartDate] = useState(today());
  const [endDate, setEndDate] = useState(today());
  const [destination, setDestination] = useState(initial.current.destination ?? "");
  const [delaySeconds, setDelaySeconds] = useState(initial.current.delaySeconds ?? 15);
  const [optimize, setOptimize] = useState(initial.current.optimize ?? true);
  const [optimizationQuality, setOptimizationQuality] = useState(() =>
    preferredQuality(
      initial.current.optimizationQuality,
      initial.current.compressionStrength,
      initial.current.profile
    )
  );
  const [keepOriginal, setKeepOriginal] = useState(initial.current.keepOriginal ?? false);
  const [optimizationMode, setOptimizationMode] = useState<"auto" | "manual">(initial.current.optimizationMode ?? "auto");
  const [workerCeiling, setWorkerCeiling] = useState(initial.current.workerCeiling ?? 16);
  const [cronTime, setCronTime] = useState(initial.current.cronTime ?? "07:00");
  const [queueSection, setQueueSection] = useState<NewspaperQueueSection>("queue");
  const [draggedJobId, setDraggedJobId] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [savingSchedule, setSavingSchedule] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [isPausingAll, setIsPausingAll] = useState(false);
  const [jobProgress, setJobProgress] = useState<NewspaperJobProgress[]>([]);
  const [optimizationRuntime, setOptimizationRuntime] = useState<OptimizationRuntime>({
    active: false,
    mode: "auto",
    requestedWorkers: workerCeiling,
    admittedWorkers: 0,
    activeWorkers: 0,
    memorySafe: true
  });

  const optimizationProfile = optimizationQuality >= 89 ? "webp_high" : "webp_balanced";
  const compressionLabel = optimizationQuality >= 89
    ? "Light"
    : optimizationQuality >= 70
      ? "Balanced"
      : optimizationQuality >= 50
        ? "Strong"
        : optimizationQuality >= 35
          ? "Compact"
          : "Archive";
  async function refresh() {
    if (!isTauriRuntime()) return;
    try {
      const state = await invoke<Bootstrap>("bootstrap_newspaper_state");
      setCatalog(state.catalog.length ? state.catalog : FALLBACK_CATALOG);
      setJobs(state.jobs);
      setBatches(state.batches ?? []);
      setSchedules(state.schedules);
    } catch (error) {
      toast.error("Could not load newspaper state", { description: String(error) });
    }
  }

  useEffect(() => {
    if (mode === "library") return;
    let disposed = false;
    let activityTimer: number | undefined;
    let unlistenProgress: (() => void) | undefined;
    const pollActivity = async () => {
      if (disposed || !isTauriRuntime()) return;
      let nextDelay = 15_000;
      try {
        const snapshot = await invoke<ActivitySnapshot>("get_newspaper_activity_snapshot");
        if (disposed) return;
        setJobs(snapshot.jobs);
        setBatches(snapshot.batches ?? []);
        setJobProgress(snapshot.progress);
        setSchedules(snapshot.schedules);
        setOptimizationRuntime(snapshot.optimizationRuntime);
        nextDelay = snapshot.hasLiveActivity ? 1_000 : 15_000;
      } catch {
        nextDelay = 15_000;
      }
      if (!disposed) activityTimer = window.setTimeout(() => void pollActivity(), nextDelay);
    };
    if (isTauriRuntime()) {
      void listen("newspaper://optimization-progress", () => {
        if (disposed) return;
        if (activityTimer !== undefined) window.clearTimeout(activityTimer);
        activityTimer = window.setTimeout(() => void pollActivity(), 100);
      }).then((unlisten) => {
        if (disposed) unlisten();
        else unlistenProgress = unlisten;
      });
      void invoke<NewspaperEdition[]>("refresh_newspaper_catalog")
        .then((items) => items.length && setCatalog(items))
        .catch(() => undefined)
        .finally(() => void refresh().finally(() => {
          if (!disposed) activityTimer = window.setTimeout(() => void pollActivity(), 1_000);
        }));
    } else {
      void refresh();
    }
    return () => {
      disposed = true;
      unlistenProgress?.();
      if (activityTimer !== undefined) window.clearTimeout(activityTimer);
    };
  }, [mode]);

  useEffect(() => {
    window.localStorage.setItem(PREF_KEY, JSON.stringify({
      destination,
      delaySeconds,
      optimize,
      optimizationQuality,
      optimizationMode,
      workerCeiling,
      keepOriginal,
      cronTime,
      selected: [...selected]
    }));
  }, [cronTime, delaySeconds, destination, keepOriginal, optimizationMode, optimizationQuality, optimize, selected, workerCeiling]);

  const progressByJob = useMemo(
    () => new Map(jobProgress.map((item) => [item.jobId, item])),
    [jobProgress]
  );
  const aggregateProgress = useMemo(() => jobProgress.reduce((aggregate, item) => ({
    downloaded: aggregate.downloaded + item.downloadCompleted,
    downloadTotal: aggregate.downloadTotal + item.downloadTotal,
    optimized: aggregate.optimized + item.optimizationCompleted,
    optimizationTotal: aggregate.optimizationTotal + item.optimizationTotal
  }), { downloaded: 0, downloadTotal: 0, optimized: 0, optimizationTotal: 0 }), [jobProgress]);

  const visibleEditions = useMemo(
    () => catalog.filter((edition) => kind === "all" || edition.kind === kind),
    [catalog, kind],
  );
  const visibleJobs = useMemo(
    () => jobs.filter((job) => !job.dismissed),
    [jobs]
  );
  const batchById = useMemo(
    () => new Map(batches.map((batch) => [batch.id, batch])),
    [batches]
  );
  const queuedJobs = useMemo(() => {
    const now = Math.floor(Date.now() / 1000);
    const scheduledAtFor = (job: NewspaperJob) => {
      const scheduledAt = batchById.get(job.batch_id)?.scheduled_at;
      return typeof scheduledAt === "number" && scheduledAt > now ? scheduledAt : null;
    };
    return visibleJobs
      .filter((job) => job.status === "queued")
      .sort((left, right) => {
        const leftScheduled = scheduledAtFor(left) != null;
        const rightScheduled = scheduledAtFor(right) != null;
        if (leftScheduled !== rightScheduled) return leftScheduled ? 1 : -1;
        return left.queue_position - right.queue_position;
      });
  }, [visibleJobs, batchById]);
  const activeJobs = useMemo(
    () => visibleJobs
      .filter((job) => job.status === "active" || job.status === "optimizing")
      .sort((left, right) => left.queue_position - right.queue_position),
    [visibleJobs]
  );
  const completedJobs = useMemo(
    () => visibleJobs
      .filter((job) => job.status === "completed")
      .sort((left, right) => (right.completed_at ?? right.updated_at) - (left.completed_at ?? left.updated_at)),
    [visibleJobs]
  );
  const failedJobs = useMemo(
    () => visibleJobs
      .filter((job) => ["failed", "partial", "unavailable", "cancelled"].includes(job.status))
      .sort((left, right) => (right.completed_at ?? right.updated_at) - (left.completed_at ?? left.updated_at)),
    [visibleJobs]
  );
  const progressJobs = queueSection === "queue"
    ? queuedJobs
    : queueSection === "active"
      ? activeJobs
      : queueSection === "completed"
        ? completedJobs
        : failedJobs;
  const queueTabCount = queuedJobs.length + schedules.length;

  function futureScheduledAt(job: NewspaperJob) {
    const scheduledAt = batchById.get(job.batch_id)?.scheduled_at;
    const now = Math.floor(Date.now() / 1000);
    return typeof scheduledAt === "number" && scheduledAt > now ? scheduledAt : null;
  }

  async function chooseFolder() {
    const picked = await open({ directory: true, multiple: false, title: "Choose newspaper folder" });
    if (typeof picked === "string" && picked.trim()) {
      writeNewspaperDestination(picked);
      setDestination(picked);
    }
  }

  async function ensureNewspaperDestination(): Promise<string | null> {
    return ensureDestination({
      current: parseDestination(destination),
      ask: async () => {
        const picked = await open({ directory: true, multiple: false, title: "Choose newspaper folder" });
        if (typeof picked !== "string" || !picked.trim()) {
          return null;
        }
        writeNewspaperDestination(picked);
        setDestination(picked);
        return picked;
      }
    });
  }

  async function validateSelection(): Promise<string | null> {
    const resolvedDestination = await ensureNewspaperDestination();
    if (!resolvedDestination) {
      toast.warning("Choose a download folder");
      return null;
    }
    if (selected.size === 0) {
      toast.warning("Select at least one edition");
      return null;
    }
    if (!isTauriRuntime()) {
      toast.info("Browser preview", { description: "Run the Tauri app to download newspapers." });
      return null;
    }
    return resolvedDestination;
  }

  async function saveSchedule() {
    if (dateMode === "custom") {
      toast.warning("Custom ranges cannot repeat daily", {
        description: "Choose Single date or Last 7 days before adding a schedule."
      });
      return;
    }
    const resolvedDestination = await validateSelection();
    if (!resolvedDestination) return;
    setSavingSchedule(true);
    try {
      await invoke("create_newspaper_schedule", {
        request: {
          editionCodes: [...selected],
          cronTime,
          destination: resolvedDestination,
          dateMode,
          delaySeconds,
          optimizeImages: optimize,
          optimizationProfile,
          optimizationQuality,
          keepOriginalJpg: keepOriginal
        }
      });
      toast.success(`Daily schedule saved for ${cronTime}`, {
        description: dateMode === "last7_days"
          ? "The latest seven local calendar days will be checked on every run."
          : "The current local calendar date will be checked on every run."
      });
      await refresh();
    } catch (error) {
      toast.error("Could not save newspaper schedule", { description: String(error) });
    } finally {
      setSavingSchedule(false);
    }
  }

  async function submitDownload() {
    const resolvedDestination = await validateSelection();
    if (!resolvedDestination) return;
    setSubmitting(true);
    try {
      const response = await invoke<CreateBatchResponse>("create_newspaper_batch", {
        request: {
          editionCodes: [...selected],
          dateMode,
          startDate: dateMode === "last7_days" ? today() : startDate,
          endDate: dateMode === "custom" ? endDate : undefined,
          destination: resolvedDestination,
          delaySeconds,
          optimizeImages: optimize,
          optimizationProfile,
          optimizationQuality,
          keepOriginalJpg: keepOriginal
        }
      });
      if (response.jobs.length === 0 && response.skippedCount > 0) {
        toast.info("Newspapers already exist", {
          description: `${response.skippedCount} edition${response.skippedCount === 1 ? "" : "s"} skipped or resumed.`
        });
      } else {
        const queued = response.jobs.length;
        toast.success(`${queued} newspaper download${queued === 1 ? "" : "s"} queued`, {
          description: response.skippedCount > 0
            ? `${response.skippedCount} existing edition${response.skippedCount === 1 ? "" : "s"} skipped.`
            : dateMode === "last7_days"
              ? "Seven-day download confirmed. Image optimization will start as soon as the first edition finishes downloading."
              : "Image optimization will start as soon as this edition finishes downloading."
        });
      }
      await refresh();
      setProcessing(true);
      await onRequestQueueProcess?.(buildOptimizationRunOptions());
      await refresh();
    } catch (error) {
      toast.error("Could not start newspaper download", { description: String(error) });
    } finally {
      setSubmitting(false);
      setProcessing(false);
    }
  }

  async function toggleSchedule(item: NewspaperSchedule) {
    await invoke("toggle_newspaper_schedule", {
      scheduleId: item.id,
      enabled: !item.enabled
    });
    await refresh();
  }

  async function deleteSchedule(item: NewspaperSchedule) {
    if (!window.confirm(`Remove the ${formatClockTime(item.cron_time)} daily schedule? Pending automatic retries from this schedule will stop. Downloaded newspapers will not be affected.`)) return;
    await invoke("delete_newspaper_schedule", { scheduleId: item.id });
    toast.success("Daily newspaper schedule removed");
    await refresh();
  }

  function buildOptimizationRunOptions() {
    const preferences = readNewspaperOptimizationPreferences();
    return {
      mode: optimizationMode,
      workerCeiling,
      workerMemoryBudgetMb: preferences.workerMemoryBudgetMb,
      memoryReserveBytes: preferences.memoryReserveMb * 1024 * 1024
    };
  }

  function continueQueue() {
    if (!isTauriRuntime() || !onRequestQueueProcess) return;
    setProcessing(true);
    const options = buildOptimizationRunOptions();
    void Promise.resolve(onRequestQueueProcess(options))
      .catch((error: unknown) => toast.error("Could not continue newspaper queue", { description: String(error) }))
      .finally(() => {
        setProcessing(false);
        void refresh();
      });
  }

  async function toggleJobPause(job: NewspaperJob) {
    if (!isTauriRuntime()) return;
    try {
      await invoke("set_newspaper_job_pause", { jobId: job.id, paused: !job.paused });
      toast.success(job.paused ? "Download resumed" : "Download paused", {
        description: job.paused
          ? "The saved queue position and completed pages will be reused."
          : "Completed pages remain validated and the pause survives restart."
      });
      await refresh();
      if (job.paused) continueQueue();
    } catch (error) {
      toast.error("Could not update download", { description: String(error) });
    }
  }

  // Visible queue: anything still doing work, regardless of its current pause
  // flag. Excludes terminal states and dismissed editions so the Download-now
  // slot can revert to a fresh start when the queue drains.
  const pausableNewspaperJobs = jobs.filter(
    (job) =>
      !job.dismissed &&
      (job.status === "active" || job.status === "queued" || job.status === "optimizing")
  );
  const allNewspaperJobsPaused =
    pausableNewspaperJobs.length > 0 && pausableNewspaperJobs.every((job) => job.paused);
  const isNewspaperQueueRunning = pausableNewspaperJobs.length > 0;

  async function toggleAllNewspaperJobsPause() {
    if (!isTauriRuntime() || pausableNewspaperJobs.length === 0) return;
    const nextPaused = !allNewspaperJobsPaused;
    setIsPausingAll(true);
    try {
      const updatedIds = await invoke<string[]>("set_all_newspaper_jobs_paused", {
        paused: nextPaused
      });
      toast.info(nextPaused ? "All newspaper downloads paused" : "All newspaper downloads resumed", {
        description: nextPaused
          ? "Active work will pause at the next safe boundary. Queued and scheduled editions will wait."
          : "Queued editions are available to continue."
      });
      await refresh();
      // The cooperative pause signal in the Rust worker may not have unwound
      // the in-flight process_newspaper_queue task yet; re-arm it on resume so
      // the worker picks up where it left off, mirroring the LinkedIn flow.
      if (!nextPaused) continueQueue();
      if (updatedIds.length === 0) {
        // nothing was pausable at the moment of the call; refresh already covered
        // any visible drift, so the UI settles back to Download naturally.
      }
    } catch (error) {
      toast.error(nextPaused ? "Pause all failed" : "Resume all failed", {
        description: String(error)
      });
    } finally {
      setIsPausingAll(false);
    }
  }

  async function startQueuedJob(job: NewspaperJob) {
    if (!isTauriRuntime()) return;
    try {
      if (job.paused) {
        await invoke("set_newspaper_job_pause", { jobId: job.id, paused: false });
      }
      const queuedIds = jobs
        .filter((item) => !item.dismissed)
        .filter((item) => item.status === "queued")
        .sort((left, right) => left.queue_position - right.queue_position)
        .map((item) => item.id);
      await invoke("reorder_newspaper_jobs", {
        jobIds: [job.id, ...queuedIds.filter((id) => id !== job.id)]
      });
      await refresh();
      continueQueue();
    } catch (error) {
      toast.error("Could not start queued download", { description: String(error) });
    }
  }

  async function retryJob(job: NewspaperJob) {
    if (!isTauriRuntime()) return;
    try {
      await invoke("retry_newspaper_job", { jobId: job.id });
      toast.success("Missing newspaper pages queued again");
      await refresh();
      continueQueue();
    } catch (error) {
      toast.error("Could not retry newspaper", { description: String(error) });
    }
  }

  async function removeJob(job: NewspaperJob) {
    const prompt = ["completed", "partial"].includes(job.status)
      ? "Permanently delete this downloaded edition? Its local newspaper files and progress history will be removed."
      : "Permanently delete this queue item and its saved files? This cannot be undone.";
    if (!window.confirm(prompt) || !isTauriRuntime()) return;
    try {
      await invoke("remove_newspaper_job", { jobId: job.id });
      toast.success("Newspaper edition deleted", {
        description: "Its local files and progress history were removed."
      });
      await refresh();
    } catch (error) {
      toast.error("Could not delete newspaper edition", { description: String(error) });
    }
  }

  function handleQueueDragStart(event: ReactDragEvent<HTMLButtonElement>, job: NewspaperJob) {
    if (job.status !== "queued") {
      event.preventDefault();
      return;
    }
    setDraggedJobId(job.id);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", job.id);
  }

  async function handleQueueDrop(event: ReactDragEvent<HTMLElement>, target: NewspaperJob) {
    event.preventDefault();
    const sourceId = draggedJobId ?? event.dataTransfer.getData("text/plain");
    setDraggedJobId(null);
    if (!sourceId || sourceId === target.id || target.status !== "queued") return;
    const queuedIds = jobs
      .filter((job) => !job.dismissed && job.status === "queued")
      .sort((left, right) => left.queue_position - right.queue_position)
      .map((job) => job.id);
    const from = queuedIds.indexOf(sourceId);
    const to = queuedIds.indexOf(target.id);
    if (from < 0 || to < 0) return;
    queuedIds.splice(from, 1);
    queuedIds.splice(to, 0, sourceId);
    try {
      await invoke("reorder_newspaper_jobs", { jobIds: queuedIds });
      await refresh();
    } catch (error) {
      toast.error("Could not reorder queue", { description: String(error) });
    }
  }

  if (mode === "library") {
    return (
      <NewspaperLibrary
        clippingCapability={{ enabled: true, onCreated: onOpenClipping }}
        onReaderTargetConsumed={onReaderTargetConsumed}
        onReturnClipping={onReturnClipping ?? onOpenClipping}
        readerTarget={readerTarget}
      />
    );
  }

  return (
    <section className="newspaper-download" aria-label="Download World Journal editions">
      <div className="lv-workspace newspaper-downloads-workspace">
        <div className="newspaper-search-stage">
          <section className="newspaper-editions" aria-label="Select newspaper editions">
            <div className="newspaper-edition-tabs" role="tablist" aria-label="Edition groups">
              {([["all", "All"], ["daily", "Regional"], ["weekly", "Weekly"], ["special", "Special"]] as const).map(([value, label]) => (
                <button type="button" role="tab" aria-selected={kind === value} className={kind === value ? "active" : undefined} key={value} onClick={() => setKind(value)}>
                  {label}
                </button>
              ))}
            </div>
            <div className="newspaper-edition-list">
              {visibleEditions.map((edition) => {
                const key = editionKey(edition);
                return (
                  <div className="newspaper-edition-row" key={key}>
                    <Checkbox label="" checked={selected.has(key)} aria-label={`Select ${edition.nameZh} ${edition.code}`} onChange={(event) => {
                      const next = new Set(selected);
                      if (event.target.checked) next.add(key); else next.delete(key);
                      setSelected(next);
                    }} />
                    <span>{edition.nameZh}<small>{edition.nameEn}</small></span>
                    <em>{edition.code}</em>
                  </div>
                );
              })}
              {visibleEditions.length === 0 ? <div className="newspaper-empty">No editions in this group.</div> : null}
            </div>
            <footer className="newspaper-edition-footer">
              <span>{selected.size} selected</span>
              <button type="button" onClick={() => {
                const allVisibleSelected = visibleEditions.every((edition) => selected.has(editionKey(edition)));
                const next = new Set(selected);
                visibleEditions.forEach((edition) => {
                  const key = editionKey(edition);
                  if (allVisibleSelected) next.delete(key); else next.add(key);
                });
                setSelected(next);
              }}>
                {visibleEditions.length > 0 && visibleEditions.every((edition) => selected.has(editionKey(edition))) ? "Clear" : "Select all"}
              </button>
            </footer>
          </section>

          <div className="newspaper-control-cluster" aria-label="Newspaper download settings">
            <label className="newspaper-cluster-field newspaper-option-date">
              <span>
                Dates
                <Tooltip label="Uses the system date automatically and includes today.">
                  <button type="button" className="newspaper-setting-help" aria-label="About the Last 7 days date range">
                    <CircleHelp aria-hidden="true" />
                  </button>
                </Tooltip>
              </span>
              <Select value={dateMode} onChange={(event) => setDateMode(event.target.value as typeof dateMode)}>
                <option value="single">Single</option>
                <option value="last7_days">Last 7</option>
                <option value="custom">Custom</option>
              </Select>
            </label>
            <div className={`newspaper-cluster-field newspaper-option-when${dateMode === "last7_days" ? " is-system-date" : ""}`}>
              <span>{dateMode === "custom" ? "Span" : "Day"}</span>
              <div className="newspaper-date-controls">
                <Input type="date" value={dateMode === "last7_days" ? today() : startDate} onChange={(event) => setStartDate(event.target.value)} disabled={dateMode === "last7_days"} aria-label={dateMode === "last7_days" ? "System current date" : "Start publication date"} />
                {dateMode === "custom" ? <Input type="date" value={endDate} onChange={(event) => setEndDate(event.target.value)} aria-label="End publication date" /> : null}
              </div>
            </div>
            <label className="newspaper-cluster-field newspaper-option-delay">
              <span>Delay</span>
              <div className="newspaper-delay-field">
                <Input type="number" min={0} max={3600} value={delaySeconds} onChange={(event) => setDelaySeconds(Number(event.target.value))} aria-label="Edition delay in seconds" className="newspaper-delay-input" />
                <span className="newspaper-delay-unit" aria-hidden="true">sec</span>
              </div>
            </label>
          </div>

          <div className={`newspaper-options-row${optimize ? "" : " is-disabled"}`} aria-label="Image optimization">
            <div className="newspaper-optimization-toggles">
              <Switch label="Optimize" checked={optimize} onChange={(event) => setOptimize(event.target.checked)} />
              <Checkbox label="Keep JPG" checked={keepOriginal} onChange={(event) => setKeepOriginal(event.target.checked)} disabled={!optimize} />
              <Tooltip label={`${compressionLabel} · Quality 25 may soften fine print. JPG remains only if WebP is larger or fails.${optimizationRuntime.limitedReason ? ` ${optimizationRuntime.limitedReason}` : ""}`}>
                <button type="button" className="newspaper-setting-help" aria-label="About newspaper image optimization">
                  <CircleHelp aria-hidden="true" />
                </button>
              </Tooltip>
            </div>
            <label className="newspaper-cluster-field newspaper-option-quality">
              <span>Quality</span>
              <Select value={String(optimizationQuality)} onChange={(event) => setOptimizationQuality(Number(event.target.value))} disabled={!optimize} aria-label="Image compression quality">
                <option value="92">92</option>
                <option value="86">86</option>
                <option value="74">74</option>
                <option value="55">55</option>
                <option value="45">45</option>
                <option value="35">35</option>
                <option value="25">25</option>
              </Select>
            </label>
            <label className="newspaper-cluster-field newspaper-option-workers">
              <span>Workers</span>
              <Select value={optimizationMode} onChange={(event) => setOptimizationMode(event.target.value as "auto" | "manual")} disabled={!optimize} aria-label="Optimization worker mode">
                <option value="auto">Auto</option>
                <option value="manual">Manual</option>
              </Select>
            </label>
            <label className="newspaper-cluster-field newspaper-option-ceiling">
              <span>Max</span>
              <Select value={String(workerCeiling)} onChange={(event) => setWorkerCeiling(Number(event.target.value))} disabled={!optimize || optimizationMode === "auto"} aria-label="Optimization worker ceiling">
                {[2, 4, 8, 12, 16, 20].map((workers) => <option value={workers} key={workers}>{workers}</option>)}
              </Select>
            </label>
          </div>

          <div className="newspaper-action-row" aria-label="Folder, schedule, and download">
            <label className="newspaper-cluster-field newspaper-cluster-folder">
              <span>Folder</span>
              <button
                type="button"
                className="newspaper-folder-field"
                onClick={() => void chooseFolder()}
                aria-label="Browse newspaper folder"
                title={destination || "Choose folder"}
              >
                <Folder aria-hidden="true" />
                <span className="newspaper-folder-path">{destination || "Choose folder"}</span>
              </button>
            </label>
            <label className="newspaper-cluster-field newspaper-option-time">
              <span>Schedule</span>
              <Input type="time" value={cronTime} onChange={(event) => setCronTime(event.target.value)} aria-label="Daily newspaper schedule time" />
            </label>
            <Button type="button" variant="outline" className="newspaper-action-button newspaper-action-schedule" loading={savingSchedule} onClick={() => void saveSchedule()}>
              <CalendarClock aria-hidden="true" className="h-3.5 w-3.5" />
              Add schedule
            </Button>
            <Button type="button" variant="primary" className="newspaper-action-button newspaper-action-download" loading={submitting || processing} onClick={() => void submitDownload()}>
              <Download aria-hidden="true" className="h-3.5 w-3.5" />
              Download
            </Button>
          </div>
        </div>

        <section className="newspaper-queue-panel" aria-label="Newspaper download progress">
          <div className="queue-section-tabs newspaper-queue-section-tabs" role="group" aria-label="Newspaper download queue sections">
            <NewspaperQueueSectionTab
              section="queue"
              label="Queue"
              value={queueTabCount}
              tone="queue"
              selected={queueSection === "queue"}
              onClick={() => setQueueSection("queue")}
            />
            <NewspaperQueueSectionTab
              section="active"
              label="Active"
              value={activeJobs.length}
              tone="primary"
              selected={queueSection === "active"}
              onClick={() => setQueueSection("active")}
            />
            <NewspaperQueueSectionTab
              section="completed"
              label="Completed"
              value={completedJobs.length}
              tone="success"
              selected={queueSection === "completed"}
              onClick={() => setQueueSection("completed")}
            />
            <NewspaperQueueSectionTab
              section="failed"
              label="Failed"
              value={failedJobs.length}
              tone="danger"
              selected={queueSection === "failed"}
              onClick={() => setQueueSection("failed")}
            />
          </div>
          <div className="table-panel-header newspaper-queue-section-header">
            <div className="table-panel-header-status newspaper-queue-header-actions">
              {isNewspaperQueueRunning ? (
                <div className="newspaper-queue-controls" aria-label="Newspaper queue controls">
                  <Button
                    type="button"
                    size="xs"
                    variant="outline"
                    loading={isPausingAll}
                    onClick={() => void toggleAllNewspaperJobsPause()}
                    disabled={isPausingAll}
                  >
                    {allNewspaperJobsPaused
                      ? <Play aria-hidden="true" className="h-3.5 w-3.5" />
                      : <Pause aria-hidden="true" className="h-3.5 w-3.5" />}
                    {isPausingAll
                      ? "Updating"
                      : allNewspaperJobsPaused
                        ? "Resume all"
                        : "Pause all"}
                  </Button>
                </div>
              ) : null}
            </div>
          </div>
          <div className={`newspaper-progress-table queue-section-panel queue-section-panel-${queueSection}`} aria-label={`${queueSection} newspaper downloads`}>
            {queueSection === "queue" && progressJobs.length === 0 && schedules.length === 0 ? (
              <div className="newspaper-empty">Queued editions will appear here.</div>
            ) : queueSection !== "queue" && progressJobs.length === 0 ? (
              <div className="newspaper-empty">{`No ${queueSection} editions`}</div>
            ) : (
              <>
                {queueSection === "queue"
                  ? schedules.map((schedule) => (
                    <article className={`newspaper-progress-row newspaper-schedule-queue-row${schedule.enabled ? "" : " is-paused"}`} key={`schedule:${schedule.id}`}>
                      <span className="newspaper-schedule-queue-mark" aria-hidden="true">
                        <CalendarClock />
                      </span>
                      <div className="newspaper-progress-edition">
                        <strong>{scheduleEditionSummary(schedule, catalog)}</strong>
                        <span>
                          Daily {formatClockTime(schedule.cron_time)}
                          {" · "}
                          {schedule.edition_codes.length} edition{schedule.edition_codes.length === 1 ? "" : "s"}
                          {" · "}
                          {scheduleDateModeLabel(schedule.date_mode)}
                        </span>
                      </div>
                      <div className="newspaper-progress-status">
                        <StatusBadge className={schedule.enabled ? "scheduled-status-pill" : undefined} tone={schedule.enabled ? "primary" : "neutral"}>
                          {schedule.enabled ? "Scheduled" : "Paused"}
                        </StatusBadge>
                        <span>Does not block downloads</span>
                      </div>
                      <div className="newspaper-job-progress newspaper-schedule-queue-copy">
                        <span className="newspaper-job-progress-percent">—</span>
                        <div className="newspaper-schedule-queue-bar" aria-hidden="true"><i /></div>
                        <div className="newspaper-stage-counts"><span>Repeats every day at local time</span></div>
                      </div>
                      <div className="newspaper-progress-actions">
                        <button type="button" aria-label={schedule.enabled ? "Pause daily schedule" : "Resume daily schedule"} title={schedule.enabled ? "Pause schedule" : "Resume schedule"} onClick={() => void toggleSchedule(schedule)}>{schedule.enabled ? <Pause /> : <Play />}</button>
                        <button type="button" className="danger" aria-label="Delete daily schedule" title="Delete schedule" onClick={() => void deleteSchedule(schedule)}><Trash2 /></button>
                      </div>
                    </article>
                  ))
                  : null}
                {progressJobs.map((job) => {
              const details = progressByJob.get(job.id);
              const progress = exactProgressPercent(job, details);
              const awaitingRelease = Boolean(job.retry_at && job.retry_at * 1000 > Date.now());
              const scheduledAt = futureScheduledAt(job);
              const scheduled = scheduledAt != null;
              return (
                <article
                  className={`newspaper-progress-row${draggedJobId === job.id ? " is-dragging" : ""}${scheduled ? " is-scheduled" : ""}`}
                  key={job.id}
                  onDragOver={(event) => {
                    if (job.status === "queued" && !scheduled) {
                      event.preventDefault();
                      event.dataTransfer.dropEffect = "move";
                    }
                  }}
                  onDrop={(event) => void handleQueueDrop(event, job)}
                >
                  <button
                    type="button"
                    className="newspaper-drag-handle"
                    draggable={job.status === "queued" && !scheduled}
                    disabled={job.status !== "queued" || scheduled}
                    aria-label={`Reorder ${job.edition_name}`}
                    title={scheduled ? "Scheduled editions stay after immediate downloads" : job.status === "queued" ? "Drag to reorder" : "Only queued items can be reordered"}
                    onDragStart={(event) => handleQueueDragStart(event, job)}
                    onDragEnd={() => setDraggedJobId(null)}
                  >
                    <GripVertical />
                  </button>
                  <div className="newspaper-progress-edition"><strong>{job.edition_name} · {job.edition_code}</strong><span>{job.publication_date}</span></div>
                  <div className="newspaper-progress-status">
                    <StatusBadge
                      className={scheduled ? "scheduled-status-pill" : job.status === "completed" ? "is-completed" : undefined}
                      tone={scheduled ? "primary" : job.status === "completed" ? "success" : job.status === "failed" || job.status === "partial" ? "danger" : job.paused || awaitingRelease ? "neutral" : "primary"}
                    >
                      {scheduled
                        ? "Scheduled"
                        : awaitingRelease
                          ? "Awaiting release"
                          : formatJobStatus(job, details)}
                    </StatusBadge>
                    <span>{scheduled && scheduledAt ? formatScheduledTime(scheduledAt) : formatJobTime(job)}</span>
                  </div>
                  <div className="newspaper-job-progress">
                    <span className="newspaper-job-progress-percent">{scheduled || job.status === "queued" || job.paused ? "—" : `${progress}%`}</span>
                    <div role="progressbar" aria-label={`${job.edition_name} download progress`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={scheduled ? 0 : progress}>
                      <i style={{ width: `${scheduled ? 0 : progress}%` }} />
                    </div>
                    {scheduled ? (
                      <div className="newspaper-stage-counts"><span>Waits until scheduled time · does not block downloads</span></div>
                    ) : (() => {
                      if (!details) return <div className="newspaper-stage-counts"><span>Waiting</span></div>;
                      const hasFailure = details.downloadFailed > 0 || details.optimizationFailed > 0;
                      if (hasFailure) {
                        return (
                          <div className="newspaper-stage-counts is-failed" role="status">
                            {details.downloadFailed > 0 ? (
                              <span><em>Failed</em> {details.downloadFailed} download{details.downloadFailed === 1 ? "" : "s"}</span>
                            ) : null}
                            {details.optimizationFailed > 0 ? (
                              <span><em>Failed</em> {details.optimizationFailed} optimization{details.optimizationFailed === 1 ? "" : "s"}</span>
                            ) : null}
                          </div>
                        );
                      }
                      return (
                        <div className="newspaper-stage-counts">
                          <span><em>Downloaded</em> {details.downloadCompleted}/{details.downloadTotal}</span>
                          {details.optimizationTotal > 0 ? (
                            <span><em>Optimized</em> {details.optimizationCompleted}/{details.optimizationTotal}</span>
                          ) : null}
                        </div>
                      );
                    })()}
                  </div>
                  <div className="newspaper-progress-actions">
                    {job.status === "active" || job.status === "optimizing" ? (
                      <button type="button" aria-label={`Pause ${job.edition_name}`} title="Pause download" onClick={() => void toggleJobPause(job)}><Pause /></button>
                    ) : null}
                    {job.status === "queued" ? (
                      <button type="button" aria-label={`${scheduled ? "Start now" : job.paused ? "Resume" : "Start"} ${job.edition_name}`} title={scheduled ? "Start now instead of waiting" : job.paused ? "Resume download" : "Start this download next"} onClick={() => void startQueuedJob(job)}><Play /></button>
                    ) : null}
                    {["partial", "failed", "unavailable"].includes(job.status) ? (
                      <button type="button" aria-label={`Retry ${job.edition_name}`} title="Retry missing pages" onClick={() => void retryJob(job)}><RotateCcw /></button>
                    ) : null}
                    {["completed", "partial"].includes(job.status) ? (
                      <button type="button" aria-label={`Open ${job.edition_name} folder`} title="Open download folder" onClick={() => void invoke("open_newspaper_download_folder", { path: job.output_dir })}><FolderOpen /></button>
                    ) : null}
                    <button
                      type="button"
                      className="danger"
                      aria-label={`Delete ${job.edition_name} and its local files`}
                      title={["active", "optimizing"].includes(job.status) ? "Pause this download before deleting it" : "Delete edition and local files"}
                      disabled={["active", "optimizing"].includes(job.status)}
                      onClick={() => void removeJob(job)}
                    ><Trash2 /></button>
                  </div>
                </article>
              );
            })}
              </>
            )}
          </div>
        </section>
      </div>
      {processing || optimizationRuntime.active ? (
        <div className="newspaper-processing">
          <LoaderCircle className="lv-button-spinner" />
          <span>
            Downloaded {aggregateProgress.downloaded}/{aggregateProgress.downloadTotal}
            {aggregateProgress.optimizationTotal > 0 ? ` · Optimized ${aggregateProgress.optimized}/${aggregateProgress.optimizationTotal}` : ""}
            {optimizationRuntime.active ? ` · ${optimizationRuntime.activeWorkers}/${optimizationRuntime.admittedWorkers} workers${optimizationRuntime.cpuPercent != null ? ` · CPU ${Math.round(optimizationRuntime.cpuPercent)}%` : ""}` : ""}
          </span>
        </div>
      ) : null}
    </section>
  );
}

function NewspaperQueueSectionTab({
  section,
  label,
  value,
  tone,
  selected,
  onClick
}: {
  section: NewspaperQueueSection;
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
      <span className="queue-section-tab-value">{value}</span>
    </button>
  );
}

function formatJobStatus(job: NewspaperJob, progress?: NewspaperJobProgress) {
  if (job.paused) return "Paused";
  if (progress?.currentStage === "finalizing") return "Finalizing";
  if (progress?.currentStage === "optimizing" && progress.optimizationRecovered > 0) return "Resuming";
  const labels: Record<string, string> = {
    active: "Downloading",
    queued: "Queued",
    optimizing: "Optimizing",
    completed: "Completed",
    partial: "Needs retry",
    failed: "Failed",
    unavailable: "Unavailable",
    cancelled: "Cancelled"
  };
  return labels[job.status] ?? job.status;
}

function exactProgressPercent(job: NewspaperJob, progress?: NewspaperJobProgress) {
  if (job.status === "completed" || progress?.currentStage === "complete") return 100;
  if (!progress) {
    const terminal = job.completed_count + job.failed_count;
    return job.page_count > 0 ? Math.min(99, Math.round((terminal / job.page_count) * 100)) : 0;
  }
  const downloadTerminal = progress.downloadCompleted + progress.downloadFailed;
  const optimizationTerminal = progress.optimizationCompleted + progress.optimizationFailed;
  const downloadDone = progress.downloadTotal > 0 && downloadTerminal >= progress.downloadTotal;
  const isOptimizationStage = progress.optimizationTotal > 0 && downloadDone;
  const terminal = isOptimizationStage ? optimizationTerminal : downloadTerminal;
  const total = isOptimizationStage ? progress.optimizationTotal : progress.downloadTotal;
  return total > 0 ? Math.min(99, Math.round((terminal / total) * 100)) : 0;
}

function formatJobTime(job: NewspaperJob) {
  const timestamp = job.completed_at ?? job.updated_at ?? job.created_at;
  if (!timestamp) return "Time unavailable";
  return new Date(timestamp * 1000).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit"
  });
}

function formatScheduledTime(timestamp: number) {
  return new Date(timestamp * 1000).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit"
  });
}

function formatClockTime(value: string) {
  const [hours, minutes] = value.split(":").map(Number);
  if (!Number.isFinite(hours) || !Number.isFinite(minutes)) return value;
  const date = new Date();
  date.setHours(hours, minutes, 0, 0);
  return date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function scheduleEditionSummary(schedule: NewspaperSchedule, catalog: NewspaperEdition[]) {
  const names = schedule.edition_codes.map((key) => {
    const edition = catalog.find((item) => editionKey(item) === key || item.code === key.split("@")[0]);
    return edition ? `${edition.nameZh} · ${edition.code}` : key;
  });
  if (names.length <= 2) return names.join(", ");
  return `${names.slice(0, 2).join(", ")} +${names.length - 2}`;
}

function scheduleDateModeLabel(dateMode: NewspaperSchedule["date_mode"]) {
  return dateMode === "last7_days" ? "Last 7 days" : "Single date";
}

function readPreferences(): {
  destination?: string;
  delaySeconds?: number;
  compressionStrength?: number;
  optimizationQuality?: number;
  optimize?: boolean;
  profile?: string;
  keepOriginal?: boolean;
  optimizationMode?: "auto" | "manual";
  workerCeiling?: number;
  cronTime?: string;
  selected?: string[];
} {
  try {
    return JSON.parse(window.localStorage.getItem(PREF_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function preferredQuality(quality?: number, compressionStrength?: number, profile?: string) {
  if (quality !== undefined && WEBP_QUALITY_PRESETS.includes(quality)) return quality;
  if (compressionStrength === 0) return 92;
  if (compressionStrength === 16) return 86;
  if (compressionStrength === 50) return 74;
  if (compressionStrength === 100) return 55;
  if (profile === "webp_high") return 92;
  if (profile === "webp_balanced") return 86;
  return 74;
}
