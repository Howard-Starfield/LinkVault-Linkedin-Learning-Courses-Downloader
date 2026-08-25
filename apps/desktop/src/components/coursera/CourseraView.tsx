import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  CircleHelp,
  Eye,
  EyeOff,
  Folder,
  FolderOpen,
  History,
  KeyRound,
  Play,
  RotateCcw,
  Trash2,
  X
} from "lucide-react";
import { IconCertificate } from "@tabler/icons-react";
import {
  Button,
  Checkbox,
  DataTable,
  DataTableHeader,
  DataTableRow,
  Dialog,
  EmptyRow,
  IconButton,
  Input,
  Panel,
  Progress,
  Select,
  Textarea,
  Tooltip,
  guardedToast
} from "../primitives";
import {
  bootstrapCourseraState,
  cancelActiveCourseraDownload,
  clearFailedCourseraJobs,
  clearSavedCourseraToken,
  fetchCourseraSyllabusPreview,
  hasSavedCourseraToken,
  loadCourseraPreferences,
  openCourseraDownloadFolder,
  removeFailedCourseraJob,
  retryFailedCourseraJob,
  saveCourseraPreferences,
  saveCourseraToken,
  startCourseraDownloadJobs
} from "../../lib/coursera/ipc";
import { parseCourseraArtifactCounts } from "../../lib/coursera/types";
import type {
  AuthMethodKind,
  CourseraBootstrapState,
  CourseraHistoryEntry,
  CourseraJob,
  ParsedCourseraClass,
  ProcessCourseraResponse,
  SavedCourseraPreferences,
  SyllabusPreview
} from "../../lib/coursera/types";

/** Matches `.coursera-search-input` min/max-height; keep in sync with CSS. */
const COURSERA_URL_MIN_HEIGHT_PX = 40;
const COURSERA_URL_MAX_HEIGHT_PX = 132;
const SAVED_CAUTH_PLACEHOLDER = "••••••••••••••••";
const COURSERA_PREFS_STORAGE_KEY = "linkvault.coursera.preferences";
const COURSERA_RESOLUTIONS = ["360p", "540p", "720p"] as const;
type CourseraResolution = (typeof COURSERA_RESOLUTIONS)[number];
type CourseraQueueSection = "queue" | "active" | "completed" | "failed";

const EMPTY_PREFS: SavedCourseraPreferences = {
  outputDir: "",
  selectedResolution: "540p",
  formats: [],
  ignoredFormats: [],
  subtitleLanguage: "all",
  downloadQuizzes: false,
  downloadNotebooks: false,
  downloadAbout: false,
  resume: false,
  overwrite: false,
  generatePlaylists: false,
  sectionFilter: "",
  lectureFilter: "",
  resourceFilter: "",
  jobs: 1,
  downloadDelaySeconds: 60
};

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function writeLocalPrefs(prefs: SavedCourseraPreferences): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(COURSERA_PREFS_STORAGE_KEY, JSON.stringify(prefs));
}

function asResolution(value: string): CourseraResolution {
  return (COURSERA_RESOLUTIONS as readonly string[]).includes(value)
    ? (value as CourseraResolution)
    : "540p";
}

function clampPositiveInt(value: number, min: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(min, Math.floor(value));
}

export function CourseraView({ mode = "downloads" }: { mode?: "downloads" | "history" }) {
  // --- form state ---------------------------------------------------------
  const [classInput, setClassInput] = useState("");
  const [parsedClasses, setParsedClasses] = useState<ParsedCourseraClass[]>([]);
  const [selectedSlugs, setSelectedSlugs] = useState<string[]>([]);
  const [outputDir, setOutputDir] = useState("");
  const [resolution, setResolution] = useState<CourseraResolution>("540p");
  const [subtitleLanguage, setSubtitleLanguage] = useState("all");
  const [formatsText, setFormatsText] = useState("");
  const [ignoredFormatsText, setIgnoredFormatsText] = useState("");
  const [sectionFilter, setSectionFilter] = useState("");
  const [lectureFilter, setLectureFilter] = useState("");
  const [resourceFilter, setResourceFilter] = useState("");
  const [downloadQuizzes, setDownloadQuizzes] = useState(false);
  const [downloadNotebooks, setDownloadNotebooks] = useState(false);
  const [downloadAbout, setDownloadAbout] = useState(false);
  const [resume, setResume] = useState(false);
  const [overwrite, setOverwrite] = useState(false);
  const [generatePlaylists, setGeneratePlaylists] = useState(false);
  const [parallelJobs, setParallelJobs] = useState(1);
  const [delaySeconds, setDelaySeconds] = useState(60);

  // --- auth state ---------------------------------------------------------
  const [hasSavedToken, setHasSavedToken] = useState(false);
  const [authMethod, setAuthMethod] = useState<AuthMethodKind>("saved_token");
  const [authEmail, setAuthEmail] = useState("");
  const [authPassword, setAuthPassword] = useState("");
  const [cauthValue, setCauthValue] = useState("");
  const [showCauth, setShowCauth] = useState(false);
  const [isAuthenticating, setIsAuthenticating] = useState(false);
  const [, setAuthStatus] = useState<"signed_out" | "signed_in" | "saving" | "clearing">(
    "signed_out"
  );
  const [authHelpOpen, setAuthHelpOpen] = useState(false);

  // --- run state ----------------------------------------------------------
  const [jobs, setJobs] = useState<CourseraJob[]>([]);
  const [history, setHistory] = useState<CourseraHistoryEntry[]>([]);
  const [isStarting, setIsStarting] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const classInputRef = useRef<HTMLTextAreaElement | null>(null);
  const [clearingJobId, setClearingJobId] = useState<string | null>(null);
  const [queueSection, setQueueSection] = useState<CourseraQueueSection>("queue");
  const [isSavingPrefs, setIsSavingPrefs] = useState(false);
  const [previewSyllabus, setPreviewSyllabus] = useState<SyllabusPreview | null>(null);
  const [previewSlug, setPreviewSlug] = useState<string | null>(null);
  const [isPreviewingSyllabus, setIsPreviewingSyllabus] = useState(false);
  const cancellationRef = useRef(false);

  // --- bootstrap on mount -------------------------------------------------
  useEffect(() => {
    void refreshAll();
  }, []);

  async function refreshAll(): Promise<void> {
    try {
      const state = await bootstrapCourseraState();
      applyBootstrapState(state);
    } catch (error) {
      // Browser previews (no Tauri) still surface a sensible state.
      if (isTauriRuntime()) {
        toast.error("Coursera bootstrap failed", { description: String(error) });
      }
    }
    try {
      const token = await hasSavedCourseraToken();
      setHasSavedToken(token);
    } catch {
      // ignore — token store may not exist yet
    }
  }

  function applyBootstrapState(state: CourseraBootstrapState): void {
    const prefs = state.savedPrefs ?? state.defaultOptions ?? EMPTY_PREFS;
    applyPreferences(prefs);
    setJobs(state.persistedJobs ?? []);
    setHistory(state.downloadHistory ?? []);
    setHasSavedToken(Boolean(state.hasSavedToken));
  }

  function applyPreferences(prefs: SavedCourseraPreferences): void {
    const safe: SavedCourseraPreferences = { ...EMPTY_PREFS, ...prefs };
    setOutputDir(safe.outputDir);
    setResolution(asResolution(safe.selectedResolution));
    setSubtitleLanguage(safe.subtitleLanguage);
    setFormatsText(safe.formats.join(" "));
    setIgnoredFormatsText(safe.ignoredFormats.join(" "));
    setSectionFilter(safe.sectionFilter);
    setLectureFilter(safe.lectureFilter);
    setResourceFilter(safe.resourceFilter);
    setDownloadQuizzes(safe.downloadQuizzes);
    setDownloadNotebooks(safe.downloadNotebooks);
    setDownloadAbout(safe.downloadAbout);
    setResume(safe.resume);
    setOverwrite(safe.overwrite);
    setGeneratePlaylists(safe.generatePlaylists);
    setParallelJobs(clampPositiveInt(safe.jobs, 1, 1));
    setDelaySeconds(clampPositiveInt(safe.downloadDelaySeconds, 0, 60));
  }

  function currentPreferences(): SavedCourseraPreferences {
    return {
      outputDir: outputDir.trim(),
      selectedResolution: resolution,
      formats: splitList(formatsText),
      ignoredFormats: splitList(ignoredFormatsText),
      subtitleLanguage: subtitleLanguage.trim() || "all",
      downloadQuizzes,
      downloadNotebooks,
      downloadAbout,
      resume,
      overwrite,
      generatePlaylists,
      sectionFilter: sectionFilter.trim(),
      lectureFilter: lectureFilter.trim(),
      resourceFilter: resourceFilter.trim(),
      jobs: clampPositiveInt(parallelJobs, 1, 1),
      downloadDelaySeconds: clampPositiveInt(delaySeconds, 0, 0)
    };
  }

  // --- derived state ------------------------------------------------------
  const liveJobs = useMemo(
    () => jobs.filter((job) => isLiveStatus(job.status)),
    [jobs]
  );
  const completedJobs = useMemo(
    () =>
      jobs
        .filter((job) => job.status.toLowerCase() === "completed")
        .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0)),
    [jobs]
  );
  const activeJobs = useMemo(
    () => jobs.filter((job) => job.status.toLowerCase() === "active"),
    [jobs]
  );
  const failedJobs = useMemo(
    () => jobs.filter((job) => {
      const status = job.status.toLowerCase();
      return status === "failed" || status === "cancelled";
    }),
    [jobs]
  );

  const queueCounts = {
    active: activeJobs.length,
    completed: completedJobs.length,
    failed: failedJobs.length
  };

  const canStart = useMemo(() => {
    const hasContent =
      selectedSlugs.length > 0 || (classInput.trim().length > 0 && parsedClasses.length > 0);
    return (
      hasContent &&
      outputDir.trim().length > 0 &&
      (hasSavedToken || cauthValue.trim().length > 0 || (authEmail.trim() && authPassword.trim())) &&
      !isStarting
    );
  }, [selectedSlugs, classInput, parsedClasses, outputDir, hasSavedToken, cauthValue, authEmail, authPassword, isStarting]);

  const syncCourseraUrlHeight = useCallback(() => {
    const el = classInputRef.current;
    if (!el) return;
    el.style.height = "0px";
    const next = Math.min(
      Math.max(el.scrollHeight, COURSERA_URL_MIN_HEIGHT_PX),
      COURSERA_URL_MAX_HEIGHT_PX
    );
    el.style.height = `${next}px`;
  }, []);

  useLayoutEffect(() => {
    syncCourseraUrlHeight();
  }, [classInput, syncCourseraUrlHeight]);

  // --- handlers -----------------------------------------------------------
  function parseInput(): ParsedCourseraClass[] {
    const lines = classInput
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (lines.length === 0) {
      setParsedClasses([]);
      setSelectedSlugs([]);
      return [];
    }
    const out: ParsedCourseraClass[] = [];
    for (const line of lines) {
      out.push(parseCourseraLine(line));
    }
    setParsedClasses(out);
    setSelectedSlugs(out.map((entry) => entry.slug));
    return out;
  }

  async function startDownload() {
    const parsed = parsedClasses.length > 0 ? parsedClasses : parseInput();
    if (parsed.length === 0) {
      toast.warning("Course slug required", {
        description: "Paste at least one Coursera course slug or /learn/<slug> URL."
      });
      return;
    }

    const slugs =
      selectedSlugs.length > 0
        ? selectedSlugs.filter((slug) => parsed.some((course) => course.slug === slug))
        : parsed.map((course) => course.slug);

    if (slugs.length === 0) {
      toast.warning("No classes selected", {
        description: "Select at least one parsed class before starting the download."
      });
      return;
    }

    if (!outputDir.trim()) {
      toast.warning("Output folder required", { description: "Choose where to save the downloads." });
      return;
    }

    const completedSlugs = new Set(history.map((entry) => entry.job.className));
    const alreadyDownloaded = slugs.filter((slug) => completedSlugs.has(slug));
    let forceRedownload = false;
    if (alreadyDownloaded.length > 0) {
      const shouldDownloadAgain = window.confirm(
        `LinkVault has already completed ${alreadyDownloaded.length} selected Coursera course${alreadyDownloaded.length === 1 ? "" : "s"}:\n\n${alreadyDownloaded.join("\n")}\n\nDownload ${alreadyDownloaded.length === 1 ? "it" : "them"} again?`
      );
      if (!shouldDownloadAgain) return;
      forceRedownload = true;
    }

    try {
      setIsStarting(true);
      cancellationRef.current = false;

      // Resolve the auth method we'll use for this run.
      const method = await ensureAuthBeforeRun();
      if (!method) {
        setIsStarting(false);
        return;
      }

      const prefs = currentPreferences();
      writeLocalPrefs(prefs);
      // Best-effort persist; ignore failures in preview runtime.
      try {
        await saveCourseraPreferences(prefs);
      } catch {
        // ignore
      }

      const response = await startCourseraDownloadJobs({
        classes: slugs,
        outputDir: prefs.outputDir,
        forceRedownload,
        selectedResolution: prefs.selectedResolution,
        formats: prefs.formats,
        ignoredFormats: prefs.ignoredFormats,
        subtitleLanguage: prefs.subtitleLanguage,
        downloadQuizzes: prefs.downloadQuizzes,
        downloadNotebooks: prefs.downloadNotebooks,
        downloadAbout: prefs.downloadAbout,
        resume: prefs.resume,
        overwrite: prefs.overwrite,
        generatePlaylists: prefs.generatePlaylists,
        sectionFilter: prefs.sectionFilter,
        lectureFilter: prefs.lectureFilter,
        resourceFilter: prefs.resourceFilter,
        jobs: prefs.jobs,
        downloadDelaySeconds: prefs.downloadDelaySeconds
      });

      setJobs(response);
      setParsedClasses([]);
      setClassInput("");
      setSelectedSlugs([]);
      toast.success("Coursera queue updated", {
        description: `${response.length} course${response.length === 1 ? "" : "s"} persisted to the local queue.`
      });

      const processResponse = await waitForQueueIdle();
      await refreshAll();
      if (processResponse.processed) {
        showProcessedToast(processResponse);
      } else {
        toast.info("No queued Coursera download to process");
      }
    } catch (error) {
      await refreshAll();
      toast.error("Coursera download failed", { description: String(error) });
    } finally {
      setIsStarting(false);
    }
  }

  async function ensureAuthBeforeRun(): Promise<AuthMethodKind | null> {
    if (hasSavedToken) {
      // Use whatever the user already saved.
      return "saved_token";
    }
    if (cauthValue.trim()) {
      // Persist and use the pasted CAUTH.
      setAuthStatus("saving");
      try {
        await saveCourseraToken({ cauth: cauthValue.trim(), email: authEmail.trim() });
        setHasSavedToken(true);
        setAuthStatus("signed_in");
        setCauthValue("");
        return "cauth";
      } catch (error) {
        setAuthStatus("signed_out");
        toast.error("CAUTH save failed", { description: String(error) });
        return null;
      }
    }
    if (authEmail.trim() && authPassword.trim()) {
      setAuthStatus("saving");
      try {
        const session = await invoke<{ email: string; cauthSet: boolean }>("coursera_login", {
          req: {
            kind: "email_password",
            email: authEmail.trim(),
            password: authPassword
          }
        });
        setHasSavedToken(Boolean(session.cauthSet));
        setAuthStatus("signed_in");
        setAuthPassword("");
        return "email_password";
      } catch (error) {
        setAuthStatus("signed_out");
        toast.error("Coursera sign-in failed", { description: String(error) });
        return null;
      }
    }
    toast.warning("Coursera sign-in required", {
      description: "Paste a CAUTH cookie, enter email + password, or sign in once via the auth panel."
    });
    return null;
  }

  async function waitForQueueIdle(): Promise<ProcessCourseraResponse> {
    let summary: ProcessCourseraResponse = emptyProcessResponse();
    for (let i = 0; i < 120; i += 1) {
      if (cancellationRef.current) break;
      const state = await bootstrapCourseraState();
      setJobs(state.persistedJobs);
      const busy = state.persistedJobs.some((job) => {
        const status = job.status.toLowerCase();
        return status === "queued" || status === "active";
      });
      if (!busy) {
        summary.processed = true;
        return summary;
      }
      await sleep(500);
    }
    return summary;
  }

  async function cancelDownload() {
    if (!isStarting) return;
    cancellationRef.current = true;
    setIsCancelling(true);
    try {
      await cancelActiveCourseraDownload();
      toast.info("Cancellation requested");
    } catch (error) {
      toast.error("Cancellation failed", { description: String(error) });
    } finally {
      setIsCancelling(false);
    }
  }

  async function retryJob(job: CourseraJob) {
    if (job.status.toLowerCase() !== "failed") return;
    try {
      setIsStarting(true);
      const updated = await retryFailedCourseraJob(job.id);
      setJobs((prev) => [updated, ...prev.filter((candidate) => candidate.id !== job.id)]);
      toast.info("Retry queued", { description: job.className });
      await waitForQueueIdle();
      await refreshAll();
    } catch (error) {
      toast.error("Retry failed", { description: String(error) });
    } finally {
      setIsStarting(false);
    }
  }

  async function clearFailed() {
    try {
      const removed = await clearFailedCourseraJobs();
      toast.info("Failed queue cleared", { description: `${removed} item(s) removed.` });
      await refreshAll();
    } catch (error) {
      toast.error("Clear failed", { description: String(error) });
    }
  }

  async function clearFailedJob(job: CourseraJob) {
    const status = job.status.toLowerCase();
    if (status !== "failed" && status !== "cancelled") return;
    const shouldClear = window.confirm(
      `Clear the failed attempt for ${job.className}? This removes the task record and its activity log. Partial downloaded files will remain.`
    );
    if (!shouldClear) return;

    setClearingJobId(job.id);
    try {
      await removeFailedCourseraJob(job.id);
      setJobs((previous) => previous.filter((candidate) => candidate.id !== job.id));
      toast.info("Failed attempt cleared", { description: job.className });
    } catch (error) {
      toast.error("Clear failed attempt", { description: String(error) });
      await refreshAll();
    } finally {
      setClearingJobId(null);
    }
  }

  async function savePrefs() {
    setIsSavingPrefs(true);
    try {
      const prefs = currentPreferences();
      await saveCourseraPreferences(prefs);
      writeLocalPrefs(prefs);
      try {
        const persisted = await loadCourseraPreferences();
        applyPreferences(persisted);
      } catch {
        // ignore — server may not be reachable in preview
      }
      toast.success("Coursera preferences saved");
    } catch (error) {
      toast.error("Save preferences failed", { description: String(error) });
    } finally {
      setIsSavingPrefs(false);
    }
  }

  async function browseOutputFolder() {
    if (!isTauriRuntime()) {
      guardedToast("Folder picker unavailable in preview", "The native picker is only available in the Tauri desktop runtime.");
      return;
    }
    try {
      const selected = await open({ directory: true, multiple: false, defaultPath: outputDir || undefined });
      if (typeof selected === "string" && selected.trim()) {
        setOutputDir(selected);
        toast.success("Output folder selected", { description: selected });
      }
    } catch (error) {
      toast.error("Folder picker failed", { description: String(error) });
    }
  }

  async function clearCauth() {
    setAuthStatus("clearing");
    try {
      await clearSavedCourseraToken();
      setHasSavedToken(false);
      setCauthValue("");
      setAuthPassword("");
      toast.info("Saved CAUTH cleared");
    } catch (error) {
      toast.error("Clear failed", { description: String(error) });
    } finally {
      setAuthStatus("signed_out");
    }
  }

  async function previewSyllabusFor(slug: string) {
    setPreviewSlug(slug);
    setPreviewSyllabus(null);
    setIsPreviewingSyllabus(true);
    try {
      const preview = await fetchCourseraSyllabusPreview(slug);
      setPreviewSyllabus(preview);
    } catch (error) {
      toast.error("Syllabus preview failed", { description: String(error) });
      setPreviewSlug(null);
    } finally {
      setIsPreviewingSyllabus(false);
    }
  }

  async function openJobFolder(job: CourseraJob) {
    if (!job.outputDir) {
      toast.warning("Folder unavailable");
      return;
    }
    try {
      const opened = await openCourseraDownloadFolder(job.id);
      toast.success("Folder opened", { description: opened });
    } catch (error) {
      // Fall back to a friendly toast — preview runtime cannot open folders.
      toast.info("Folder opener is only available in the Tauri desktop runtime", {
        description: job.outputDir
      });
      void error;
    }
  }

  async function copyCourseraUrl(job: CourseraJob) {
    const url = `https://www.coursera.org/learn/${job.className}`;
    try {
      await navigator.clipboard.writeText(url);
      toast.success("Course URL copied", { description: url });
    } catch (error) {
      toast.error("Could not copy course URL", { description: String(error) });
    }
  }

  if (mode === "history") {
    return <CourseraHistoryPage entries={history} onOpenFolder={openJobFolder} />;
  }

  return (
    <>
      <div className="lv-workspace coursera-downloads-workspace">
        <div className="coursera-search-stage">
          <div className="coursera-search-field">
            <Textarea
              ref={classInputRef}
              value={classInput}
              onChange={(event) => {
                setClassInput(event.target.value);
                if (parsedClasses.length > 0) {
                  setParsedClasses([]);
                  setSelectedSlugs([]);
                }
              }}
              onBlur={() => parseInput()}
              placeholder="Paste Coursera course URLs or slugs"
              spellCheck={false}
              rows={1}
              className="coursera-search-input"
              aria-label="Coursera course slugs"
            />
          </div>

          {parsedClasses.length > 0 ? (
            <div className="parsed-classes coursera-parsed-classes">
              <div className="parsed-classes-list">
                {parsedClasses.map((course) => {
                  const checked = selectedSlugs.includes(course.slug);
                  return (
                    <label key={`${course.slug}-${course.original}`} className="parsed-class-chip">
                      <Checkbox
                        checked={checked}
                        onChange={(event) => {
                          if (event.target.checked) {
                            setSelectedSlugs((prev) => Array.from(new Set([...prev, course.slug])));
                          } else {
                            setSelectedSlugs((prev) => prev.filter((slug) => slug !== course.slug));
                          }
                        }}
                        label={course.slug}
                      />
                      <Tooltip label="Show syllabus preview">
                        <IconButton
                          size="icon-sm"
                          aria-label={`Preview ${course.slug}`}
                          onClick={() => previewSyllabusFor(course.slug)}
                        >
                          <IconCertificate aria-hidden="true" className="h-3.5 w-3.5" />
                        </IconButton>
                      </Tooltip>
                    </label>
                  );
                })}
              </div>
            </div>
          ) : null}

          <div className="coursera-control-cluster">
            <label className="coursera-cluster-field coursera-option-quality">
              <span>Quality</span>
              <Select value={resolution} onChange={(event) => setResolution(asResolution(event.target.value))} aria-label="Coursera video resolution">
                {COURSERA_RESOLUTIONS.map((value) => (
                  <option key={value} value={value}>{value}</option>
                ))}
              </Select>
            </label>
            <label className="coursera-cluster-field coursera-option-delay">
              <span>Delay</span>
              <div className="coursera-delay-field">
                <Input
                  value={delaySeconds}
                  type="number"
                  min={0}
                  onChange={(event) => setDelaySeconds(clampPositiveInt(Number(event.target.value), 0, 0))}
                  aria-label="Coursera download delay"
                  className="coursera-delay-input"
                />
                <span className="coursera-delay-unit" aria-hidden="true">sec</span>
              </div>
            </label>
            <label className="coursera-cluster-field coursera-cluster-folder">
              <span>Save to</span>
              <button
                type="button"
                className="coursera-folder-field"
                onClick={() => void browseOutputFolder()}
                aria-label="Coursera output folder"
                title={outputDir || "Choose a folder"}
              >
                <Folder aria-hidden="true" />
                <span className="coursera-folder-path">{outputDir || "Choose a folder"}</span>
              </button>
            </label>
            <label className="coursera-cluster-field coursera-cluster-session">
              <span>Session</span>
              <div className="coursera-session-field">
                {authMethod === "email_password" ? (
                  <Input
                    value=""
                    readOnly
                    placeholder="Sign in with email below"
                    aria-label="Coursera email session"
                    className="coursera-session-input"
                  />
                ) : authMethod === "cauth" ? (
                  <Input
                    value={cauthValue}
                    onChange={(event) => setCauthValue(event.target.value)}
                    placeholder={hasSavedToken ? SAVED_CAUTH_PLACEHOLDER : "Paste CAUTH cookie"}
                    type={showCauth ? "text" : "password"}
                    autoComplete="off"
                    spellCheck={false}
                    aria-label="Coursera CAUTH"
                    className="coursera-session-input"
                    data-has-saved={hasSavedToken && cauthValue.length === 0 ? "true" : undefined}
                  />
                ) : (
                  <Input
                    value={hasSavedToken ? SAVED_CAUTH_PLACEHOLDER : ""}
                    readOnly
                    placeholder="Saved CAUTH required"
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    aria-label="Coursera saved CAUTH"
                    className="coursera-session-input"
                    data-has-saved={hasSavedToken ? "true" : undefined}
                    title={hasSavedToken ? "Saved Coursera session is available" : "Paste a CAUTH cookie or sign in"}
                  />
                )}
                <div className="coursera-session-actions">
                  {authMethod === "cauth" ? (
                    <IconButton type="button" size="icon-sm" className="coursera-session-action" onClick={() => setShowCauth((v) => !v)} aria-label="Toggle CAUTH visibility" title={showCauth ? "Hide CAUTH" : "Show CAUTH"}>
                      {showCauth ? <EyeOff aria-hidden="true" className="h-3.5 w-3.5" /> : <Eye aria-hidden="true" className="h-3.5 w-3.5" />}
                    </IconButton>
                  ) : null}
                  <IconButton type="button" size="icon-sm" className="coursera-session-action" onClick={() => setAuthHelpOpen(true)} aria-label="Open CAUTH help" title="How to find your CAUTH cookie">
                    <CircleHelp aria-hidden="true" className="h-3.5 w-3.5" />
                  </IconButton>
                  <IconButton type="button" size="icon-sm" className="coursera-session-action" onClick={() => void clearCauth()} aria-label="Clear Coursera token" title="Clear saved session" disabled={!hasSavedToken && !cauthValue}>
                    <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                  </IconButton>
                </div>
              </div>
            </label>
          </div>

          <div className="coursera-auth-method-row">
            <label className="coursera-cluster-field coursera-option-auth">
              <span>Auth</span>
              <Select
                value={authMethod}
                onChange={(event) => setAuthMethod(event.target.value as AuthMethodKind)}
                aria-label="Coursera auth method"
              >
                <option value="saved_token">Use saved CAUTH</option>
                <option value="cauth">Paste CAUTH cookie</option>
                <option value="email_password">Email + password</option>
              </Select>
            </label>
            <label className="coursera-cluster-field coursera-option-subtitle">
              <span>Subtitles</span>
              <Input value={subtitleLanguage} onChange={(event) => setSubtitleLanguage(event.target.value)} placeholder="all" aria-label="Coursera subtitle language" />
            </label>
            <label className="coursera-cluster-field coursera-option-jobs">
              <span>Jobs</span>
              <Input value={parallelJobs} type="number" min={1} onChange={(event) => setParallelJobs(clampPositiveInt(Number(event.target.value), 1, 1))} aria-label="Coursera parallel jobs" />
            </label>
          </div>

          {authMethod === "email_password" ? (
            <div className="coursera-auth-credentials">
              <Input
                value={authEmail}
                onChange={(event) => setAuthEmail(event.target.value)}
                placeholder="email@example.com"
                type="email"
                aria-label="Coursera email"
              />
              <Input
                value={authPassword}
                onChange={(event) => setAuthPassword(event.target.value)}
                placeholder="password"
                type="password"
                aria-label="Coursera password"
              />
              <Button
                type="button"
                className="coursera-action-button"
                onClick={async () => {
                  if (!authEmail.trim() || !authPassword) {
                    toast.warning("Email and password required");
                    return;
                  }
                  setIsAuthenticating(true);
                  try {
                    await invoke("coursera_login", {
                      req: {
                        kind: "email_password",
                        email: authEmail.trim(),
                        password: authPassword
                      }
                    });
                    setHasSavedToken(true);
                    setAuthStatus("signed_in");
                    setAuthPassword("");
                    toast.success("Signed in to Coursera");
                  } catch (error) {
                    toast.error("Sign-in failed", { description: String(error) });
                    setAuthStatus("signed_out");
                  } finally {
                    setIsAuthenticating(false);
                  }
                }}
                loading={isAuthenticating}
                loadingLabel="Signing in"
              >
                <KeyRound aria-hidden="true" className="h-3.5 w-3.5" />
                Sign in
              </Button>
            </div>
          ) : null}

          <div className="coursera-filter-row">
            <label className="coursera-cluster-field">
              <span>Section</span>
              <Input value={sectionFilter} onChange={(event) => setSectionFilter(event.target.value)} placeholder="regex" aria-label="Coursera section filter" />
            </label>
            <label className="coursera-cluster-field">
              <span>Lecture</span>
              <Input value={lectureFilter} onChange={(event) => setLectureFilter(event.target.value)} placeholder="regex" aria-label="Coursera lecture filter" />
            </label>
            <label className="coursera-cluster-field">
              <span>Resource</span>
              <Input value={resourceFilter} onChange={(event) => setResourceFilter(event.target.value)} placeholder="regex" aria-label="Coursera resource filter" />
            </label>
          </div>

          <div className="coursera-format-row">
            <label className="coursera-cluster-field">
              <span>Formats</span>
              <Input value={formatsText} onChange={(event) => setFormatsText(event.target.value)} placeholder="mp4 srt pdf" aria-label="Coursera formats whitelist" />
            </label>
            <label className="coursera-cluster-field">
              <span>Ignored</span>
              <Input value={ignoredFormatsText} onChange={(event) => setIgnoredFormatsText(event.target.value)} placeholder="csv" aria-label="Coursera ignored formats" />
            </label>
          </div>

          <div className="coursera-artifact-row">
            <div className="coursera-artifact-toggles download-toggles" aria-label="Download options">
              <Checkbox checked={downloadQuizzes} onChange={(event) => setDownloadQuizzes(event.target.checked)} label="Quizzes" />
              <Checkbox checked={downloadNotebooks} onChange={(event) => setDownloadNotebooks(event.target.checked)} label="Notebooks" />
              <Checkbox checked={downloadAbout} onChange={(event) => setDownloadAbout(event.target.checked)} label="About" />
              <Checkbox checked={resume} onChange={(event) => setResume(event.target.checked)} label="Resume" />
              <Checkbox checked={overwrite} onChange={(event) => setOverwrite(event.target.checked)} label="Overwrite" />
              <Checkbox checked={generatePlaylists} onChange={(event) => setGeneratePlaylists(event.target.checked)} label="Playlists" />
            </div>
            <div className="coursera-primary-actions">
              <Button type="button" variant="primary" className="coursera-action-button" onClick={() => void startDownload()} disabled={!canStart || isStarting}>
                <Play aria-hidden="true" className="h-3.5 w-3.5" />
                {isStarting ? "Processing" : "Download"}
              </Button>
              <Button type="button" variant="outline" className="coursera-action-button" onClick={() => void savePrefs()} loading={isSavingPrefs} loadingLabel="Saving">
                <RotateCcw aria-hidden="true" className="h-3.5 w-3.5" />
                Save prefs
              </Button>
            </div>
          </div>
        </div>

        <Panel className="table-panel queue-panel coursera-queue-panel">
          <div className="table-panel-header">
            <h3>Queue</h3>
            <div className="table-panel-header-status coursera-queue-header-actions">
              {queueCounts.failed > 0 ? (
                <button type="button" className="queue-clear-button" aria-label="Clear failed queue items" onClick={clearFailed}>
                  Clear
                </button>
              ) : null}
              <div className="coursera-queue-controls" aria-label="Coursera queue controls">
                <Button type="button" size="xs" variant="outline" onClick={() => void cancelDownload()} disabled={!isStarting || isCancelling}>
                  <X aria-hidden="true" className="h-3.5 w-3.5" />
                  {isCancelling ? "Cancelling" : "Cancel all"}
                </Button>
              </div>
            </div>
          </div>
          <div className="queue-section-tabs" role="group" aria-label="Coursera download queue sections">
            <CourseraQueueSectionTab section="queue" label="Queue" value={liveJobs.length > 0 ? liveJobs.length : parsedClasses.length} tone="queue" selected={queueSection === "queue"} onClick={() => setQueueSection("queue")} />
            <CourseraQueueSectionTab section="active" label="Active" value={queueCounts.active} tone="primary" selected={queueSection === "active"} onClick={() => setQueueSection("active")} />
            <CourseraQueueSectionTab section="completed" label="Completed" value={queueCounts.completed} tone="success" selected={queueSection === "completed"} onClick={() => setQueueSection("completed")} />
            <CourseraQueueSectionTab section="failed" label="Failed" value={queueCounts.failed} tone="danger" selected={queueSection === "failed"} onClick={() => setQueueSection("failed")} />
          </div>
          <div className={`queue-section-panel queue-section-panel-${queueSection}`} aria-label={`${queueSection} Coursera downloads`}>
            <CourseraQueueTable
              jobs={queueSection === "queue" ? liveJobs : queueSection === "active" ? activeJobs : queueSection === "completed" ? completedJobs : failedJobs}
              parsedClasses={queueSection === "queue" ? parsedClasses : []}
              emptyTitle={queueSection === "queue" ? "No active downloads" : `No ${queueSection} downloads`}
              emptyDescription=""
              hideColumnHeader
              selectedSlugs={selectedSlugs}
              onToggle={(slug) => setSelectedSlugs((prev) => prev.includes(slug) ? prev.filter((s) => s !== slug) : [...prev, slug])}
              onRetry={retryJob}
              onClearFailed={clearFailedJob}
              onCopyUrl={copyCourseraUrl}
              onOpenFolder={queueSection === "completed" ? openJobFolder : undefined}
              clearingJobId={clearingJobId}
            />
          </div>
        </Panel>
      </div>

      <Dialog
        open={authHelpOpen}
        onOpenChange={setAuthHelpOpen}
        title="Find your Coursera CAUTH cookie"
        description="Use this only with a Coursera account you are allowed to access. LinkVault saves the cookie locally with Windows encryption."
      >
        <ol className="token-guide-steps">
          <li>Open coursera.org in your browser and sign in.</li>
          <li>Press F12, then open the Application tab.</li>
          <li>Under Storage, open Cookies and choose https://www.coursera.org.</li>
          <li>Find CAUTH, copy its full Value, and paste it into LinkVault.</li>
        </ol>
        <div className="token-guide-actions">
          <Button type="button" variant="primary" onClick={() => setAuthHelpOpen(false)}>
            Got it
          </Button>
        </div>
      </Dialog>

      <Dialog
        open={previewSlug !== null}
        onOpenChange={(open) => {
          if (!open) {
            setPreviewSlug(null);
            setPreviewSyllabus(null);
          }
        }}
        title={previewSlug ? `Syllabus preview: ${previewSlug}` : "Syllabus preview"}
        description="Lightweight structure check from the V2 syllabus endpoint. The full extractor is wired in the Rust orchestrator."
      >
        {isPreviewingSyllabus ? (
          <p className="text-sm text-muted">Fetching syllabus…</p>
        ) : previewSyllabus ? (
          <div className="syllabus-preview-grid">
            <PreviewStat label="Modules" value={previewSyllabus.moduleCount} />
            <PreviewStat label="Lessons" value={previewSyllabus.lessonCount} />
            <PreviewStat label="Items" value={previewSyllabus.totalItems} />
            <PreviewStat label="Quizzes" value={previewSyllabus.hasQuizzes ? "Yes" : "No"} />
            <PreviewStat label="Notebooks" value={previewSyllabus.hasNotebooks ? "Yes" : "No"} />
          </div>
        ) : (
          <p className="text-sm text-muted">No syllabus data available.</p>
        )}
      </Dialog>
    </>
  );

}

// --- helpers & sub-components --------------------------------------------

function isLiveStatus(status: string): boolean {
  const lower = status.toLowerCase();
  return lower !== "completed" && lower !== "cancelled";
}

function splitList(input: string): string[] {
  return input
    .split(/[\s,]+/)
    .map((part) => part.trim().toLowerCase())
    .filter(Boolean);
}

function parseCourseraLine(raw: string): ParsedCourseraClass {
  const trimmed = raw.trim();
  let slug = trimmed;
  if (/^https?:\/\//i.test(trimmed)) {
    try {
      const url = new URL(trimmed);
      const match = url.pathname.match(/\/learn\/([^/?#]+)/i);
      slug = match ? match[1] : trimmed;
    } catch {
      slug = trimmed;
    }
  } else if (trimmed.startsWith("coursera.org/learn/")) {
    slug = trimmed.replace(/^coursera\.org\/learn\//i, "").split(/[/?#]/)[0] ?? trimmed;
  }
  slug = slug.split(/[/?#]/)[0]?.toLowerCase() ?? "";
  return {
    original: trimmed,
    slug,
    normalizedUrl: `https://www.coursera.org/learn/${slug}`
  };
}

function emptyProcessResponse(): ProcessCourseraResponse {
  return { processed: false, completedArtifacts: 0, failedArtifacts: 0, cancelledArtifacts: 0 };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function showProcessedToast(response: ProcessCourseraResponse): void {
  const description = `${response.completedArtifacts} completed, ${response.failedArtifacts} failed, ${response.cancelledArtifacts} cancelled.`;
  if (response.failedArtifacts > 0 || response.cancelledArtifacts > 0) {
    toast.warning("Coursera queue processed with issues", { description });
    return;
  }
  toast.success("Coursera queue processed", { description });
}

function formatEventTime(timestamp: number): string {
  if (!timestamp) return "--:--";
  return new Date(timestamp * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false
  });
}

function eventTone(eventType: string): string | undefined {
  const normalized = eventType.toLowerCase();
  if (normalized.includes("failed") || normalized.includes("error")) return "danger";
  if (normalized.includes("completed") || normalized.includes("extracted")) return "success";
  if (normalized.includes("cancelled") || normalized.includes("skipped")) return "muted";
  return "primary";
}

function activityDotClass(tone?: string): string {
  if (tone === "danger") return "bg-danger";
  if (tone === "success") return "bg-success";
  if (tone === "muted") return "bg-muted";
  return "bg-primary";
}

function PreviewStat({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="syllabus-preview-stat">
      <div className="syllabus-preview-stat-value">{value}</div>
      <div className="syllabus-preview-stat-label">{label}</div>
    </div>
  );
}

function CourseraQueueSectionTab({
  section,
  label,
  value,
  tone,
  selected,
  onClick
}: {
  section: CourseraQueueSection;
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

function CourseraQueueTable({
  jobs,
  parsedClasses,
  selectedSlugs,
  onToggle,
  onRetry,
  onClearFailed,
  onCopyUrl,
  onOpenFolder,
  emptyTitle = "No active downloads",
  emptyDescription = "",
  hideColumnHeader = false,
  clearingJobId
}: {
  jobs: CourseraJob[];
  parsedClasses: ParsedCourseraClass[];
  selectedSlugs: string[];
  onToggle: (slug: string) => void;
  onRetry: (job: CourseraJob) => void | Promise<void>;
  onClearFailed: (job: CourseraJob) => void | Promise<void>;
  onCopyUrl: (job: CourseraJob) => void | Promise<void>;
  onOpenFolder?: (job: CourseraJob) => void | Promise<void>;
  emptyTitle?: string;
  emptyDescription?: string;
  hideColumnHeader?: boolean;
  clearingJobId: string | null;
}) {
  return (
    <DataTable className="queue-table">
      {hideColumnHeader ? null : (
        <DataTableHeader>
          <span>Status</span>
          <span>Course</span>
          <span>Progress</span>
          <span className="queue-actions-heading">Actions</span>
        </DataTableHeader>
      )}
      {jobs.length > 0 ? (
        jobs.map((job) => (
          <CourseraQueueJobRow
            key={job.id}
            job={job}
            onRetry={onRetry}
            onClearFailed={onClearFailed}
            onCopyUrl={onCopyUrl}
            onOpenFolder={onOpenFolder}
            clearing={clearingJobId === job.id}
          />
        ))
      ) : parsedClasses.length > 0 ? (
        parsedClasses.map((course) => {
          const checked = selectedSlugs.includes(course.slug);
          return (
            <DataTableRow key={`${course.slug}-${course.original}`} className="queue-table-row">
              <span className="coursera-queue-status-text">Parsed</span>
              <div className="table-course-cell">
                <span className="course-status-mark bg-primary" />
                <div className="min-w-0">
                  <div className="truncate font-medium" title={course.slug}>{course.slug}</div>
                  <div className="truncate text-soft" title={course.normalizedUrl}>{course.normalizedUrl}</div>
                </div>
              </div>
              <span className="text-muted">Waiting</span>
              <div className="queue-row-actions queue-selection-actions">
                <Checkbox
                  checked={checked}
                  onChange={() => onToggle(course.slug)}
                  label={checked ? "Selected" : "Select"}
                />
              </div>
            </DataTableRow>
          );
        })
      ) : (
        <EmptyRow title={emptyTitle} description={emptyDescription} />
      )}
    </DataTable>
  );
}

function CourseraQueueJobRow({
  job,
  onRetry,
  onClearFailed,
  onCopyUrl,
  onOpenFolder,
  clearing
}: {
  job: CourseraJob;
  onRetry: (job: CourseraJob) => void | Promise<void>;
  onClearFailed: (job: CourseraJob) => void | Promise<void>;
  onCopyUrl: (job: CourseraJob) => void | Promise<void>;
  onOpenFolder?: (job: CourseraJob) => void | Promise<void>;
  clearing: boolean;
}) {
  const counts = parseCourseraArtifactCounts(job.countsJson);
  const total = counts.total > 0 ? counts.total : derivedTotal(counts);
  const completed = counts.completed + counts.failed + counts.cancelled;
  const percent = total > 0 ? Math.max(0, Math.min(100, Math.round((completed / total) * 100))) : 0;
  const tone = job.status.toLowerCase();
  const canClear = tone === "failed" || tone === "cancelled";
  return (
    <DataTableRow
      className="queue-table-row"
      onContextMenu={(event) => {
        event.preventDefault();
        void onCopyUrl(job);
      }}
    >
      <div className="coursera-queue-status">
        <span className="coursera-queue-status-text">{capitalise(job.status)}</span>
        {tone === "failed" ? (
          <button
            type="button"
            className="queue-status-retry"
            aria-label={`Retry ${job.className}`}
            onClick={() => onRetry(job)}
          >
            <RotateCcw aria-hidden="true" className="h-3.5 w-3.5" />
          </button>
        ) : null}
      </div>
      <div className="table-course-cell">
        <span className={`course-status-mark ${activityDotClass(eventTone(job.status))}`} />
        <div className="min-w-0">
          <div className="truncate font-medium" title={job.className}>{job.className}</div>
          <div className="truncate text-soft" title={job.outputDir}>
            {counts.total > 0
              ? `${counts.completed} of ${counts.total} artifacts complete`
              : "Pending artifact plan"}
          </div>
        </div>
      </div>
      <div className="table-progress-cell">
        <Progress value={percent} />
        <span>{percent}%</span>
      </div>
      <div className="queue-row-actions">
        {tone === "completed" && onOpenFolder ? (
          <Tooltip label="Open output folder">
            <IconButton
              type="button"
              aria-label={`Open output folder for ${job.className}`}
              onClick={() => void onOpenFolder(job)}
              className="queue-open-folder-button"
              disabled={!job.outputDir}
            >
              <FolderOpen aria-hidden="true" className="h-3.5 w-3.5" />
            </IconButton>
          </Tooltip>
        ) : null}
        {canClear ? (
          <Tooltip label="Clear failed attempt">
            <IconButton
              type="button"
              aria-label={`Clear failed attempt for ${job.className}`}
              onClick={() => onClearFailed(job)}
              className="queue-remove-button"
              loading={clearing}
              disabled={clearing}
            >
              <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
            </IconButton>
          </Tooltip>
        ) : null}
      </div>
    </DataTableRow>
  );
}

function derivedTotal(counts: ReturnType<typeof parseCourseraArtifactCounts>): number {
  return (
    counts.videoTotal +
    counts.subtitleTotal +
    counts.quizTotal +
    counts.notebookTotal +
    counts.supplementTotal
  );
}

function CourseraHistoryTable({
  entries,
  onOpenFolder
}: {
  entries: CourseraHistoryEntry[];
  onOpenFolder: (job: CourseraJob) => void | Promise<void>;
}) {
  return (
    <DataTable className="history-table">
      {entries.map((entry) => (
        <DataTableRow key={entry.job.id} className="history-row">
          <div className="min-w-0">
            <div className="truncate font-medium" title={entry.job.className}>{entry.job.className}</div>
            <div className="truncate text-soft" title={entry.job.outputDir}>{entry.job.outputDir}</div>
          </div>
          <div className="history-date">{formatEventTime(entry.lastEventAt ?? entry.job.updatedAt)}</div>
          <Button size="sm" variant="ghost" onClick={() => onOpenFolder(entry.job)}>
            <History aria-hidden="true" className="h-3.5 w-3.5" />
            Open
          </Button>
        </DataTableRow>
      ))}
    </DataTable>
  );
}

function CourseraHistoryPage({
  entries,
  onOpenFolder
}: {
  entries: CourseraHistoryEntry[];
  onOpenFolder: (job: CourseraJob) => void | Promise<void>;
}) {
  return (
    <Panel className="history-page-panel">
      <div className="history-page-header">
        <div>
          <h3>Coursera download history</h3>
          <p>{entries.length} completed course{entries.length === 1 ? "" : "s"}</p>
        </div>
      </div>
      {entries.length > 0 ? (
        <CourseraHistoryTable entries={entries} onOpenFolder={onOpenFolder} />
      ) : (
        <DataTable className="history-table">
          <EmptyRow title="No downloaded Coursera courses" description="Completed Coursera downloads will appear here." />
        </DataTable>
      )}
    </Panel>
  );
}

function capitalise(value: string): string {
  if (!value) return value;
  return value.charAt(0).toUpperCase() + value.slice(1);
}
