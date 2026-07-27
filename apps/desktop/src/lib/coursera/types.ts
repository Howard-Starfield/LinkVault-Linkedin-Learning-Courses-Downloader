// TypeScript mirror of the Rust `coursera::*` serde structs.
// All fields use camelCase to match the Rust `#[serde(rename_all = "camelCase")]`
// boundary. Keep this file in sync with
// `apps/desktop/src-tauri/src/providers/coursera/`.

export type ParsedCourseraClass = {
  original: string;
  slug: string;
  normalizedUrl: string;
};

export type CourseraJob = {
  id: string;
  className: string;
  status: string;
  optionsJson: string;
  outputDir: string;
  createdAt: number;
  updatedAt: number;
  countsJson: string;
};

export type PersistedCourseraEvent = {
  id: number;
  jobId: string;
  eventType: string;
  payloadJson: string;
  createdAt: number;
};

export type SavedCourseraPreferences = {
  outputDir: string;
  selectedResolution: string;
  formats: string[];
  ignoredFormats: string[];
  subtitleLanguage: string;
  downloadQuizzes: boolean;
  downloadNotebooks: boolean;
  downloadAbout: boolean;
  resume: boolean;
  overwrite: boolean;
  generatePlaylists: boolean;
  sectionFilter: string;
  lectureFilter: string;
  resourceFilter: string;
  jobs: number;
  downloadDelaySeconds: number;
};

export type StartCourseraRequest = {
  classes: string[];
  outputDir: string;
  forceRedownload?: boolean;
  selectedResolution: string;
  formats: string[];
  ignoredFormats: string[];
  subtitleLanguage: string;
  downloadQuizzes: boolean;
  downloadNotebooks: boolean;
  downloadAbout: boolean;
  resume: boolean;
  overwrite: boolean;
  generatePlaylists: boolean;
  sectionFilter: string;
  lectureFilter: string;
  resourceFilter: string;
  jobs: number;
  downloadDelaySeconds: number;
};

export type CourseraSessionInfo = {
  email: string;
  cauthSet: boolean;
};

export type CourseraHistoryEntry = {
  job: CourseraJob;
  lastEventAt: number | null;
};

export type CourseraBootstrapState = {
  defaultOptions: SavedCourseraPreferences;
  hasSavedToken: boolean;
  savedPrefs: SavedCourseraPreferences | null;
  persistedJobs: CourseraJob[];
  recentEvents: PersistedCourseraEvent[];
  downloadHistory: CourseraHistoryEntry[];
};

export type ProcessCourseraResponse = {
  processed: boolean;
  completedArtifacts: number;
  failedArtifacts: number;
  cancelledArtifacts: number;
};

export type SyllabusPreview = {
  slug: string;
  moduleCount: number;
  lessonCount: number;
  totalItems: number;
  hasQuizzes: boolean;
  hasNotebooks: boolean;
};

export type AuthMethodRequest =
  | { kind: "email_password"; email: string; password: string }
  | { kind: "cauth"; cauth: string; email: string }
  | { kind: "saved_token"; email: string };

export type CourseraTokenSaveRequest = {
  cauth: string;
  email: string;
};

export type AuthMethodKind = "email_password" | "cauth" | "saved_token";

// Coarse progress counts derived from `CourseraJob.countsJson`.
// The backend persists a JSON blob, so we materialise it lazily.
export type CourseraArtifactCounts = {
  total: number;
  completed: number;
  failed: number;
  cancelled: number;
  active: number;
  pending: number;
  skipped: number;
  videoTotal: number;
  videoCompleted: number;
  subtitleTotal: number;
  subtitleCompleted: number;
  quizTotal: number;
  quizCompleted: number;
  notebookTotal: number;
  notebookCompleted: number;
  supplementTotal: number;
  supplementCompleted: number;
};

export const EMPTY_COURSERA_ARTIFACT_COUNTS: CourseraArtifactCounts = {
  total: 0,
  completed: 0,
  failed: 0,
  cancelled: 0,
  active: 0,
  pending: 0,
  skipped: 0,
  videoTotal: 0,
  videoCompleted: 0,
  subtitleTotal: 0,
  subtitleCompleted: 0,
  quizTotal: 0,
  quizCompleted: 0,
  notebookTotal: 0,
  notebookCompleted: 0,
  supplementTotal: 0,
  supplementCompleted: 0
};

export function parseCourseraArtifactCounts(countsJson: string | undefined): CourseraArtifactCounts {
  if (!countsJson) return { ...EMPTY_COURSERA_ARTIFACT_COUNTS };
  try {
    const raw = JSON.parse(countsJson) as Partial<CourseraArtifactCounts>;
    return { ...EMPTY_COURSERA_ARTIFACT_COUNTS, ...raw };
  } catch {
    return { ...EMPTY_COURSERA_ARTIFACT_COUNTS };
  }
}
