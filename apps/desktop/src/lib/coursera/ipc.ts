// Typed Tauri 2 invoke wrappers for the Coursera tab.
// Mirrors `apps/desktop/src-tauri/src/coursera/commands.rs` 1:1.
// All commands return `Result<T, String>` per Tauri 2 conventions; the
// invoke wrappers surface the error string to the caller.

import { invoke } from "@tauri-apps/api/core";
import type {
  AuthMethodRequest,
  CourseraBootstrapState,
  CourseraHistoryEntry,
  CourseraJob,
  CourseraSessionInfo,
  CourseraTokenSaveRequest,
  ParsedCourseraClass,
  ProcessCourseraResponse,
  SavedCourseraPreferences,
  StartCourseraRequest,
  SyllabusPreview
} from "./types";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function bootstrapCourseraState(): Promise<CourseraBootstrapState> {
  if (!isTauriRuntime()) {
    return previewBootstrapState();
  }
  return invoke<CourseraBootstrapState>("bootstrap_coursera_state");
}

export async function parseCourseraClassInput(input: string): Promise<ParsedCourseraClass[]> {
  if (!isTauriRuntime()) {
    return parseCourseraClassInputForPreview(input);
  }
  return invoke<ParsedCourseraClass[]>("parse_coursera_class_input", { input });
}

export async function courseraLogin(req: AuthMethodRequest): Promise<CourseraSessionInfo> {
  if (!isTauriRuntime()) {
    return previewCourseraLogin(req);
  }
  return invoke<CourseraSessionInfo>("coursera_login", { req });
}

export async function saveCourseraToken(req: CourseraTokenSaveRequest): Promise<boolean> {
  if (!isTauriRuntime()) {
    writePreviewSavedToken(req.cauth.trim().length > 0, req.email);
    return true;
  }
  return invoke<boolean>("save_coursera_token", { req });
}

export async function clearSavedCourseraToken(): Promise<boolean> {
  if (!isTauriRuntime()) {
    clearPreviewSavedToken();
    return true;
  }
  return invoke<boolean>("clear_saved_coursera_token");
}

export async function hasSavedCourseraToken(): Promise<boolean> {
  if (!isTauriRuntime()) {
    return hasPreviewSavedToken();
  }
  return invoke<boolean>("has_saved_coursera_token");
}

export async function saveCourseraPreferences(
  prefs: SavedCourseraPreferences
): Promise<boolean> {
  if (!isTauriRuntime()) {
    writePreviewPreferences(prefs);
    return true;
  }
  return invoke<boolean>("save_coursera_preferences", { prefs });
}

export async function loadCourseraPreferences(): Promise<SavedCourseraPreferences> {
  if (!isTauriRuntime()) {
    return readPreviewPreferences() ?? defaultCourseraPreferences();
  }
  return invoke<SavedCourseraPreferences>("load_coursera_preferences");
}

export async function startCourseraDownloadJobs(
  request: StartCourseraRequest
): Promise<CourseraJob[]> {
  if (!isTauriRuntime()) {
    return startCourseraJobsForPreview(request);
  }
  return invoke<CourseraJob[]>("start_coursera_download_jobs", { request });
}

export async function processNextQueuedCourseraJob(): Promise<ProcessCourseraResponse> {
  if (!isTauriRuntime()) {
    return previewProcessNext();
  }
  return invoke<ProcessCourseraResponse>("process_next_queued_coursera_job");
}

export async function processQueuedCourseraBatch(): Promise<ProcessCourseraResponse> {
  if (!isTauriRuntime()) {
    return previewProcessNext();
  }
  return invoke<ProcessCourseraResponse>("process_queued_coursera_batch", { max: 1 });
}

export async function cancelActiveCourseraDownload(): Promise<boolean> {
  if (!isTauriRuntime()) {
    return true;
  }
  return invoke<boolean>("cancel_active_coursera_download");
}

export async function retryFailedCourseraJob(jobId: string): Promise<CourseraJob> {
  if (!isTauriRuntime()) {
    throw new Error("Retry not supported in browser preview");
  }
  return invoke<CourseraJob>("retry_failed_coursera_job", { jobId });
}

export async function clearFailedCourseraJobs(): Promise<number> {
  if (!isTauriRuntime()) {
    return previewClearFailed();
  }
  return invoke<number>("clear_failed_coursera_jobs");
}

export async function listCourseraHistory(): Promise<CourseraHistoryEntry[]> {
  if (!isTauriRuntime()) {
    return readPreviewHistory();
  }
  return invoke<CourseraHistoryEntry[]>("list_coursera_history");
}

export async function openCourseraDownloadFolder(jobId: string): Promise<string> {
  if (!isTauriRuntime()) {
    throw new Error("Folder opener is unavailable in browser preview");
  }
  return invoke<string>("open_coursera_download_folder", { jobId });
}

export async function fetchCourseraSyllabusPreview(slug: string): Promise<SyllabusPreview> {
  if (!isTauriRuntime()) {
    return {
      slug,
      moduleCount: 0,
      lessonCount: 0,
      totalItems: 0,
      hasQuizzes: false,
      hasNotebooks: false
    };
  }
  return invoke<SyllabusPreview>("fetch_coursera_syllabus_preview", { slug });
}

// ---------------------------------------------------------------------------
// Browser-preview fallbacks (mirror the LinkedIn pattern in App.tsx).
// These let `pnpm dev` boot the Vite server without the Tauri runtime so
// the Coursera tab renders. Real auth/state live in the Rust commands.
// ---------------------------------------------------------------------------

const previewJobsStorageKey = "linkvault.preview.coursera.jobs";
const previewEventsStorageKey = "linkvault.preview.coursera.events";
const previewSavedTokenStorageKey = "linkvault.preview.coursera.saved-token";
const previewHistoryStorageKey = "linkvault.preview.coursera.history";
const previewPreferencesStorageKey = "linkvault.preview.coursera.preferences";

function readStoredPreviewValue<T>(value: string | null, fallback: T): T {
  if (!value) return fallback;
  try {
    return JSON.parse(value) as T;
  } catch {
    return fallback;
  }
}

function readPreviewJobs(): CourseraJob[] {
  if (typeof window === "undefined") return [];
  return readStoredPreviewValue<CourseraJob[]>(
    window.sessionStorage.getItem(previewJobsStorageKey),
    []
  );
}

function writePreviewJobs(jobs: CourseraJob[]): void {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(previewJobsStorageKey, JSON.stringify(jobs));
}

function readPreviewEvents(): PersistedCourseraLike[] {
  if (typeof window === "undefined") return [];
  return readStoredPreviewValue<PersistedCourseraLike[]>(
    window.sessionStorage.getItem(previewEventsStorageKey),
    []
  );
}

type PersistedCourseraLike = {
  id: number;
  jobId: string;
  eventType: string;
  payloadJson: string;
  createdAt: number;
};

function readPreviewHistory(): CourseraHistoryEntry[] {
  if (typeof window === "undefined") return [];
  return readStoredPreviewValue<CourseraHistoryEntry[]>(
    window.sessionStorage.getItem(previewHistoryStorageKey),
    []
  );
}

function writePreviewHistory(history: CourseraHistoryEntry[]): void {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(previewHistoryStorageKey, JSON.stringify(history));
}

function readPreviewPreferences(): SavedCourseraPreferences | null {
  if (typeof window === "undefined") return null;
  return readStoredPreviewValue<SavedCourseraPreferences | null>(
    window.sessionStorage.getItem(previewPreferencesStorageKey),
    null
  );
}

function writePreviewPreferences(prefs: SavedCourseraPreferences): void {
  if (typeof window === "undefined") return;
  window.sessionStorage.setItem(previewPreferencesStorageKey, JSON.stringify(prefs));
}

function hasPreviewSavedToken(): boolean {
  if (typeof window === "undefined") return false;
  return window.sessionStorage.getItem(previewSavedTokenStorageKey) === "true";
}

function writePreviewSavedToken(saved: boolean, email: string): void {
  if (typeof window === "undefined") return;
  if (saved) {
    window.sessionStorage.setItem(previewSavedTokenStorageKey, "true");
    window.sessionStorage.setItem("linkvault.preview.coursera.saved-email", email);
  } else {
    window.sessionStorage.removeItem(previewSavedTokenStorageKey);
    window.sessionStorage.removeItem("linkvault.preview.coursera.saved-email");
  }
}

function clearPreviewSavedToken(): void {
  if (typeof window === "undefined") return;
  window.sessionStorage.removeItem(previewSavedTokenStorageKey);
  window.sessionStorage.removeItem("linkvault.preview.coursera.saved-email");
}

function defaultCourseraPreferences(): SavedCourseraPreferences {
  return {
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
}

function previewBootstrapState(): CourseraBootstrapState {
  return {
    defaultOptions: defaultCourseraPreferences(),
    hasSavedToken: hasPreviewSavedToken(),
    savedPrefs: readPreviewPreferences(),
    persistedJobs: readPreviewJobs(),
    recentEvents: readPreviewEvents(),
    downloadHistory: readPreviewHistory()
  };
}

function parseCourseraClassInputForPreview(input: string): ParsedCourseraClass[] {
  const out: ParsedCourseraClass[] = [];
  for (const line of input.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const slug = extractCourseraSlug(trimmed);
    out.push({
      original: trimmed,
      slug,
      normalizedUrl: `https://www.coursera.org/learn/${slug}`
    });
  }
  return out;
}

function extractCourseraSlug(input: string): string {
  const withProtocol = input.startsWith("http://") || input.startsWith("https://") ? input : `https://${input}`;
  try {
    const url = new URL(withProtocol);
    const match = url.pathname.match(/\/learn\/([^/?#]+)/i);
    if (match) return match[1];
  } catch {
    // fall through
  }
  return input.replace(/[^a-z0-9-]/gi, "").toLowerCase();
}

async function previewCourseraLogin(req: AuthMethodRequest): Promise<CourseraSessionInfo> {
  const email = "email" in req && req.email ? req.email : "preview@coursera.local";
  const cauthSet =
    req.kind === "cauth"
      ? req.cauth.trim().length > 0
      : req.kind === "saved_token"
        ? hasPreviewSavedToken()
        : req.kind === "email_password"
          ? true
          : false;
  if (cauthSet) {
    writePreviewSavedToken(true, email);
  }
  return { email, cauthSet };
}

function startCourseraJobsForPreview(request: StartCourseraRequest): CourseraJob[] {
  const now = Math.floor(Date.now() / 1000);
  const jobs: CourseraJob[] = request.classes.map((slug, index) => ({
    id: `preview-coursera-${index + 1}-${slug}`,
    className: slug,
    status: "Queued",
    optionsJson: "{}",
    outputDir: request.outputDir,
    createdAt: now,
    updatedAt: now,
    countsJson: "{}"
  }));
  writePreviewJobs(jobs);
  writePreviewHistory(
    jobs.map((job) => ({ job, lastEventAt: null }))
  );
  return jobs;
}

function previewProcessNext(): ProcessCourseraResponse {
  return { processed: true, completedArtifacts: 0, failedArtifacts: 0, cancelledArtifacts: 0 };
}

function previewClearFailed(): number {
  const jobs = readPreviewJobs().filter(
    (job) => job.status.toLowerCase() !== "failed" && job.status.toLowerCase() !== "cancelled"
  );
  writePreviewJobs(jobs);
  return jobs.length;
}
