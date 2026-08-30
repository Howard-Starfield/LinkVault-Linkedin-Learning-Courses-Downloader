import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  ArrowLeft,
  CalendarClock,
  ChevronDown,
  CircleHelp,
  Clock3,
  Download,
  Folder,
  FolderOpen,
  History,
  LayoutGrid,
  List,
  Moon,
  Newspaper,
  PanelLeft,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Search,
  Settings,
  StickyNote,
  SunMedium,
  Trash2,
  X
} from "lucide-react";
import { IconBrandLinkedin, IconCertificate, IconMovie } from "@tabler/icons-react";
import liAtCookieGuide from "./assets/guide.png";
import {
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
  Textarea,
  Tooltip,
  guardedToast
} from "./components/primitives";
import { CourseraView } from "./components/coursera/CourseraView";
import { YouTubeView } from "./components/youtube/YouTubeView";
import { formatYouTubeInvokeError, startYouTubeUiMock } from "./lib/youtube/ipc";
import { ensureDestination, parseDestination } from "./lib/destinations";
import { commitLinkedInDestination } from "./lib/linkedin/ipc";
import { NewspaperView } from "./components/newspaper/NewspaperView";
import { NewspaperClippings, type ClippingFlush } from "./components/newspaper/NewspaperClippings";
import { NewspaperClippingSearch } from "./components/newspaper/NewspaperClippingSearch";
import { NewspaperSnapshotRootsSettings } from "./components/newspaper/NewspaperSnapshotRootsSettings";
import { useClippingNoteExitBridge } from "./components/newspaper/useClippingNoteExitBridge";
import { useNewspaperClippingNavigation } from "./components/newspaper/useNewspaperClippingNavigation";
import {
  NEWSPAPER_PAGE_TONES,
  NEWSPAPER_READER_ZOOM_MAX,
  NEWSPAPER_READER_ZOOM_MIN,
  NEWSPAPER_READER_ZOOM_STEP,
  clampNewspaperReaderZoom,
  readNewspaperReaderPreferences,
  writeNewspaperReaderPreferences,
  type NewspaperPageTone
} from "./components/newspaper/newspaper-reader-preferences";
import {
  NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS,
  type NewspaperOptimizationPreferences,
  type NewspaperOptimizationRunOptions,
  readNewspaperOptimizationPreferences,
  writeNewspaperOptimizationPreferences
} from "./components/newspaper/newspaper-optimization-preferences";
import {
  CLIPPING_VIEW_MODE_EVENT,
  readClippingViewMode,
  writeClippingViewMode,
  type ClippingViewMode
} from "./components/newspaper/clipping-view-preferences";

const NEWSPAPER_READER_ZOOM_OPTIONS = Array.from(
  { length: Math.round((NEWSPAPER_READER_ZOOM_MAX - NEWSPAPER_READER_ZOOM_MIN) / NEWSPAPER_READER_ZOOM_STEP) + 1 },
  (_, index) => NEWSPAPER_READER_ZOOM_MIN + index * NEWSPAPER_READER_ZOOM_STEP
);
const NEWSPAPER_PAGE_TONE_LABELS: Record<NewspaperPageTone, string> = {
  original: "Original",
  soft: "Soft paper",
  dim: "Dim paper",
  inverted: "Inverted"
};

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
  title?: string | null;
  thumbnail_url?: string | null;
  selected_quality?: string;
  output_dir?: string;
  paused?: boolean;
  scheduled_at?: number | null;
  created_at?: number;
  updated_at?: number;
  artifact_counts?: ArtifactProgressCounts;
  video_artifacts?: VideoDownloadArtifact[];
  is_test_emulator?: boolean;
};

type VideoDownloadArtifact = {
  id: string;
  display_name: string;
  status: string;
  size_bytes?: number | null;
  created_at: number;
  updated_at: number;
};

type VideoPacingState = {
  artifact_id: string;
  wait_seconds: number;
  wait_started_at: number;
  wait_until: number;
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
  study_guide_total?: number;
  study_guide_completed?: number;
  exercise_total: number;
  exercise_completed: number;
};

type PersistedJobEvent = {
  id: number;
  job_id: string;
  event_type: string;
  message: string;
  payload_json?: string | null;
  created_at: number;
};

type ActivityFilter = "active" | "completed" | "failed";
type DownloadQueueSection = "queue" | ActivityFilter;

type StartDownloadResponse = {
  jobs: QueuedDownloadJob[];
};

type StartDownloadRequest = {
  courseUrls: string;
  outputDir: string;
  selectedQuality: string;
  delaySeconds: number;
  videoWaitMinSeconds: number;
  videoWaitMaxSeconds: number;
  browserSource: string;
  downloadVideos: boolean;
  downloadExercises: boolean;
  downloadSubtitles: boolean;
  downloadQuizzes: boolean;
  schedule?: DownloadScheduleRequest;
};

type DownloadScheduleRequest = {
  windowMinutes: number;
  minWaitMinutes: number;
  maxWaitMinutes: number;
};

type AutomaticScheduleWaitRange = {
  targetWaitMinutes: number;
  minWaitMinutes: number;
  maxWaitMinutes: number;
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
  videoWaitMinSeconds?: number;
  videoWaitMaxSeconds?: number;
  browserSource: string;
  downloadVideos: boolean;
  downloadExercises: boolean;
  downloadSubtitles: boolean;
  downloadQuizzes?: boolean;
};

type DownloadHistoryEntry = {
  job_id: string;
  course_slug: string;
  source_url: string;
  course_title: string;
  output_dir: string;
  completed_at: number;
};

type BootstrapState = {
  default_resolution: string;
  browser_sources: string[];
  stores_plaintext_tokens_in_sqlite: boolean;
  has_saved_token: boolean;
  saved_download_preferences: SavedDownloadPreferences | null;
  persisted_jobs: QueuedDownloadJob[];
  recent_events: PersistedJobEvent[];
  download_history: DownloadHistoryEntry[];
  download_history_file_path: string;
};

type UpdateMetadata = {
  version: string;
  current_version: string;
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
const DOWNLOAD_DELAY_STORAGE_KEY = "linkvault.downloadDelaySeconds";
const VIDEO_WAIT_MIN_STORAGE_KEY = "linkvault.videoWaitMinSeconds";
const VIDEO_WAIT_MAX_STORAGE_KEY = "linkvault.videoWaitMaxSeconds";
const DOWNLOAD_DELAY_MAX_SECONDS = 86_400;
const VIDEO_WAIT_MAX_SECONDS = 600;
const DEFAULT_VIDEO_WAIT_MIN_SECONDS = 20;
const DEFAULT_VIDEO_WAIT_MAX_SECONDS = 40;
const TOKEN_GUIDE_DISMISSED_STORAGE_KEY = "linkvault.liAtGuideDismissed";
const THEME_STORAGE_KEY = "linkvault.theme";
const APP_VERSION = "0.2.21";
const UPDATE_TOAST_ID = "linkvault-update";
type AppTheme = "light" | "dark";
type AppView = "downloads" | "linkedin-history" | "coursera" | "coursera-history" | "newspaper-download" | "newspaper-library" | "newspaper-clippings" | "youtube" | "youtube-history";

function readInitialTheme(): AppTheme {
  if (typeof window === "undefined") return "dark";
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}
const SAVED_TOKEN_MASK = "****";
/** Matches `.linkedin-search-input` min/max-height; keep in sync with CSS. */
const LINKEDIN_URL_MIN_HEIGHT_PX = 40;
const LINKEDIN_URL_MAX_HEIGHT_PX = 132;

function clampSidebarWidth(width: number) {
  return Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH);
}

function normalizeDelaySeconds(value: unknown) {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return 0;
  return Math.min(DOWNLOAD_DELAY_MAX_SECONDS, Math.max(0, Math.round(parsed)));
}

function normalizeVideoWaitSeconds(value: unknown, fallback: number) {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(VIDEO_WAIT_MAX_SECONDS, Math.max(0, Math.round(parsed)));
}

function normalizeVideoWaitBounds(minValue: unknown, maxValue: unknown) {
  const minSeconds = normalizeVideoWaitSeconds(minValue, DEFAULT_VIDEO_WAIT_MIN_SECONDS);
  const maxSeconds = Math.max(
    minSeconds,
    normalizeVideoWaitSeconds(maxValue, DEFAULT_VIDEO_WAIT_MAX_SECONDS)
  );
  return { minSeconds, maxSeconds };
}

function readStoredDownloadDelaySeconds() {
  if (typeof window === "undefined") return null;
  const stored = window.localStorage.getItem(DOWNLOAD_DELAY_STORAGE_KEY);
  if (stored === null || stored.trim() === "") return null;
  const parsed = Number(stored);
  return Number.isFinite(parsed) ? normalizeDelaySeconds(parsed) : null;
}

function readStoredVideoWaitBounds() {
  if (typeof window === "undefined") {
    return {
      minSeconds: DEFAULT_VIDEO_WAIT_MIN_SECONDS,
      maxSeconds: DEFAULT_VIDEO_WAIT_MAX_SECONDS
    };
  }
  return normalizeVideoWaitBounds(
    window.localStorage.getItem(VIDEO_WAIT_MIN_STORAGE_KEY) ?? DEFAULT_VIDEO_WAIT_MIN_SECONDS,
    window.localStorage.getItem(VIDEO_WAIT_MAX_STORAGE_KEY) ?? DEFAULT_VIDEO_WAIT_MAX_SECONDS
  );
}

function calculateAutomaticScheduleWaitRange(windowMinutes: number, courseCount: number): AutomaticScheduleWaitRange {
  const normalizedWindowMinutes = Number.isFinite(windowMinutes)
    ? Math.min(10_080, Math.max(1, Math.round(windowMinutes)))
    : 1;
  const normalizedCourseCount = Math.max(1, Math.floor(courseCount));
  const targetWaitMinutes = normalizedWindowMinutes / normalizedCourseCount;
  const minWaitMinutes = Math.max(1, Math.min(10_080, Math.floor(targetWaitMinutes * 0.7)));
  const firstWaitCapacity = normalizedWindowMinutes - (normalizedCourseCount - 1) * minWaitMinutes;
  const maxWaitMinutes = Math.max(
    minWaitMinutes,
    Math.min(10_080, firstWaitCapacity, Math.ceil(targetWaitMinutes * 1.3))
  );

  return {
    targetWaitMinutes: Math.max(1, Math.round(targetWaitMinutes)),
    minWaitMinutes,
    maxWaitMinutes
  };
}

export default function App() {
  const initialStoredDelaySeconds = useRef(readStoredDownloadDelaySeconds());
  const initialStoredVideoWait = useRef(readStoredVideoWaitBounds());
  const initialNewspaperReaderPreferences = useRef(readNewspaperReaderPreferences());
  const [courseUrls, setCourseUrls] = useState("");
  const [folder, setFolder] = useState("");
  const [token, setToken] = useState("");
  const courseUrlsInputRef = useRef<HTMLTextAreaElement | null>(null);
  const [resolution, setResolution] = useState("720");
  const [browserSource, setBrowserSource] = useState("Chrome");
  const [browserSources, setBrowserSources] = useState(["Chrome", "Edge", "Firefox"]);
  const [delaySeconds, setDelaySeconds] = useState(initialStoredDelaySeconds.current ?? 0);
  const [videoWaitMinSeconds, setVideoWaitMinSeconds] = useState(initialStoredVideoWait.current.minSeconds);
  const [videoWaitMaxSeconds, setVideoWaitMaxSeconds] = useState(initialStoredVideoWait.current.maxSeconds);
  const [downloadVideos, setDownloadVideos] = useState(true);
  const [downloadExercises, setDownloadExercises] = useState(true);
  const [downloadSubtitles, setDownloadSubtitles] = useState(true);
  const [downloadQuizzes, setDownloadQuizzes] = useState(true);
  const [parsedCourses, setParsedCourses] = useState<ParsedCourse[]>([]);
  const [hasSavedToken, setHasSavedToken] = useState(false);
  const [queueNeedsSessionRefresh, setQueueNeedsSessionRefresh] = useState(false);
  const [isValidatingToken, setIsValidatingToken] = useState(false);
  const [isQueueingDownload, setIsQueueingDownload] = useState(false);
  const [isProcessingDownload, setIsProcessingDownload] = useState(false);
  const [isCancellingDownload, setIsCancellingDownload] = useState(false);
  const [pauseUpdatingTaskId, setPauseUpdatingTaskId] = useState<string | null>(null);
  const [isPausingAll, setIsPausingAll] = useState(false);
  const [isSavingSettings, setIsSavingSettings] = useState(false);
  const [isRegisteringNewspaperArchive, setIsRegisteringNewspaperArchive] = useState(false);
  const [isRepairingNewspaperLibrary, setIsRepairingNewspaperLibrary] = useState(false);
  const [isCheckingUpdate, setIsCheckingUpdate] = useState(false);
  const [isInstallingUpdate, setIsInstallingUpdate] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [pendingResetProvider, setPendingResetProvider] = useState<"linkedin" | "coursera" | "newspaper" | null>(null);
  const [resetInProgress, setResetInProgress] = useState<"linkedin" | "coursera" | "newspaper" | null>(null);
  const [pausingForReset, setPausingForReset] = useState<"linkedin" | "coursera" | "newspaper" | null>(null);
  const [newspaperDefaultZoom, setNewspaperDefaultZoom] = useState(initialNewspaperReaderPreferences.current.defaultZoom);
  const [newspaperClickZoom, setNewspaperClickZoom] = useState(initialNewspaperReaderPreferences.current.clickZoom);
  const [newspaperPageTone, setNewspaperPageTone] = useState<NewspaperPageTone>(initialNewspaperReaderPreferences.current.pageTone);
  const [newspaperOptimizationPreferences, setNewspaperOptimizationPreferences] =
    useState<NewspaperOptimizationPreferences>(() => readNewspaperOptimizationPreferences());
  const [isHelpOpen, setIsHelpOpen] = useState(false);
  const [isScheduleOpen, setIsScheduleOpen] = useState(false);
  const [scheduleStep, setScheduleStep] = useState<"configure" | "confirm">("configure");
  const [scheduleWindowHours, setScheduleWindowHours] = useState(6);
  const [scheduleWindowMinutes, setScheduleWindowMinutes] = useState(0);
  const [scheduleCourseCount, setScheduleCourseCount] = useState(0);
  const [isTokenGuideOpen, setIsTokenGuideOpen] = useState(false);
  const [pendingUpdate, setPendingUpdate] = useState<UpdateMetadata | null>(null);
  const [updateBannerDismissed, setUpdateBannerDismissed] = useState(false);
  const [queuedJobs, setQueuedJobs] = useState<QueuedDownloadJob[]>([]);
  const [recentEvents, setRecentEvents] = useState<PersistedJobEvent[]>([]);
  const [emulatorJobs, setEmulatorJobs] = useState<QueuedDownloadJob[]>([]);
  const [emulatorEvents, setEmulatorEvents] = useState<PersistedJobEvent[]>([]);
  const [downloadHistory, setDownloadHistory] = useState<DownloadHistoryEntry[]>([]);
  const [downloadHistoryFilePath, setDownloadHistoryFilePath] = useState("");
  const [queueSection, setQueueSection] = useState<DownloadQueueSection>("queue");
  const [activeView, setActiveView] = useState<AppView>("downloads");
  const [clippingGallerySummary, setClippingGallerySummary] = useState<{
    total: number;
    loading: boolean;
  } | null>(null);
  const [clippingViewMode, setClippingViewMode] = useState<ClippingViewMode>(() => readClippingViewMode());
  const [isClippingDetailOpen, setIsClippingDetailOpen] = useState(false);
  const [globalSearchQuery, setGlobalSearchQuery] = useState("");
  const [activeSearchQuery, setActiveSearchQuery] = useState("");
  const [isLinkedInExpanded, setIsLinkedInExpanded] = useState(true);
  const [isCourseraExpanded, setIsCourseraExpanded] = useState(true);
  const [isNewspaperExpanded, setIsNewspaperExpanded] = useState(true);
  const [isYouTubeExpanded, setIsYouTubeExpanded] = useState(true);
  const [theme, setTheme] = useState<AppTheme>(readInitialTheme);
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT_WIDTH);
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(false);
  const [isDraggingSidebar, setIsDraggingSidebar] = useState(false);
  const cancellationRequestedRef = useRef(false);
  const queueSubmissionRef = useRef(false);
  const startupUpdateCheckedRef = useRef(false);
  const downloadPreferencesHydratedRef = useRef(false);
  const downloadProcessingPromiseRef = useRef<Promise<ProcessQueuedDownloadResponse> | null>(null);
  const newspaperQueuePromiseRef = useRef<Promise<void> | null>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const sidebarDragStart = useRef({ x: 0, width: SIDEBAR_DEFAULT_WIDTH });
  const sidebarDragWidth = useRef(SIDEBAR_DEFAULT_WIDTH);
  const liveSidebarWidth = useRef(SIDEBAR_DEFAULT_WIDTH);
  const sidebarDragAnimationFrame = useRef<number | null>(null);
  const sidebarDragCleanup = useRef<(() => void) | null>(null);
  const wasSettingsOpen = useRef(false);
  const clippingFlushRef = useRef<ClippingFlush | null>(null);
  const navigationQueueRef = useRef<Promise<void>>(Promise.resolve());
  const searchRequestGenerationRef = useRef(0);
  const clippingGalleryScrollTopRef = useRef(0);
  const preSearchScrollRef = useRef(0);
  const emulatorTimersRef = useRef<number[]>([]);
  const emulatorRunRef = useRef(0);
  const scheduleWindowTotalMinutes = scheduleWindowHours * 60 + scheduleWindowMinutes;
  const automaticScheduleWaitRange = useMemo(
    () => calculateAutomaticScheduleWaitRange(scheduleWindowTotalMinutes, scheduleCourseCount),
    [scheduleWindowTotalMinutes, scheduleCourseCount]
  );
  const scheduleMinWaitMinutes = automaticScheduleWaitRange.minWaitMinutes;
  const scheduleMaxWaitMinutes = automaticScheduleWaitRange.maxWaitMinutes;

  useEffect(() => {
    return () => {
      emulatorRunRef.current += 1;
      for (const timerId of emulatorTimersRef.current) {
        window.clearTimeout(timerId);
      }
      emulatorTimersRef.current = [];
    };
  }, []);

  const registerClippingFlush = useCallback((flush: ClippingFlush | null) => {
    clippingFlushRef.current = flush;
  }, []);
  const recordClippingGalleryScroll = useCallback((scrollTop: number) => {
    clippingGalleryScrollTopRef.current = scrollTop;
  }, []);
  useClippingNoteExitBridge(isTauriRuntime(), clippingFlushRef);

  const requestNavigation = useCallback((
    nextView: AppView,
    options: { preserveClippingContext?: boolean } = {}
  ) => {
    let allowed = false;
    const task = navigationQueueRef.current.then(async () => {
      const flush = clippingFlushRef.current;
      if (flush && !(await flush())) {
        toast.error("Navigation paused", {
          description: "Your clipping draft is still unsaved. Retry the save or resolve the conflict first."
        });
        return;
      }
      if (nextView !== "newspaper-clippings" && !options.preserveClippingContext) {
        setGlobalSearchQuery("");
        setActiveSearchQuery("");
        setIsClippingDetailOpen(false);
      }
      setActiveView(nextView);
      allowed = true;
    });
    navigationQueueRef.current = task.then(() => undefined, () => undefined);
    return task.then(() => allowed);
  }, []);

  const clippingNavigation = useNewspaperClippingNavigation({
    requestNavigation,
    setDetailOpen: setIsClippingDetailOpen,
    setNewspaperExpanded: setIsNewspaperExpanded
  });

  async function updateGlobalSearch(nextRaw: string) {
    const next = [...nextRaw].slice(0, 200).join("");
    const generation = searchRequestGenerationRef.current + 1;
    searchRequestGenerationRef.current = generation;
    setGlobalSearchQuery(next);
    if (!next.trim()) {
      setActiveSearchQuery("");
      window.requestAnimationFrame(() => {
        const content = document.querySelector<HTMLElement>(".lv-content");
        if (content) content.scrollTop = preSearchScrollRef.current;
      });
      return;
    }
    if (!activeSearchQuery) {
      const content = document.querySelector<HTMLElement>(".lv-content");
      preSearchScrollRef.current = content?.scrollTop ?? 0;
      const flush = clippingFlushRef.current;
      if (flush && !(await flush())) {
        if (generation === searchRequestGenerationRef.current) setGlobalSearchQuery("");
        toast.error("Search paused", {
          description: "Save or resolve the current clipping note before leaving its editor."
        });
        return;
      }
    }
    if (generation === searchRequestGenerationRef.current) setActiveSearchQuery(next);
  }

  useEffect(() => {
    void refreshBootstrapState().then((state) => {
      if (
        isTauriRuntime() &&
        state &&
        !state.has_saved_token &&
        window.localStorage.getItem(TOKEN_GUIDE_DISMISSED_STORAGE_KEY) !== "true"
      ) {
        setIsTokenGuideOpen(true);
      }
    });
  }, []);

  useEffect(() => {
    if (startupUpdateCheckedRef.current) return;
    startupUpdateCheckedRef.current = true;
    void checkForUpdatesOnLaunch();
  }, []);

  function ensureNewspaperQueueProcessing(
    options: NewspaperOptimizationRunOptions | null = null,
    rearm = false
  ) {
    if (newspaperQueuePromiseRef.current && !rearm) {
      return newspaperQueuePromiseRef.current;
    }
    const previous = newspaperQueuePromiseRef.current ?? Promise.resolve();
    const next = previous
      .catch(() => undefined)
      .then(async () => {
        await invoke("process_newspaper_queue");
        await invoke("process_newspaper_optimization_queue", { options });
      });
    newspaperQueuePromiseRef.current = next.finally(() => {
      if (newspaperQueuePromiseRef.current === next) {
        newspaperQueuePromiseRef.current = null;
      }
    });
    return newspaperQueuePromiseRef.current;
  }

  useEffect(() => {
    if (!isTauriRuntime()) return;

    let disposed = false;
    async function processNewspaperSchedules() {
      if (disposed) return;
      try {
        await ensureNewspaperQueueProcessing(null, false);
      } catch {
        // The newspaper screen surfaces persisted job and schedule errors.
      }
    }

    void processNewspaperSchedules();
    const intervalId = window.setInterval(() => void processNewspaperSchedules(), 15_000);
    return () => {
      disposed = true;
      window.clearInterval(intervalId);
    };
  }, []);

  useEffect(() => {
    if (!hasSavedToken) return;
    let disposed = false;

    async function checkDueSchedules() {
      const state = await refreshBootstrapState();
      if (disposed || !state) return;
      if (!queueNeedsSessionRefresh && hasReadyQueuedJobs(state.persisted_jobs)) {
        ensureDownloadProcessing(true);
      }
    }

    void checkDueSchedules();
    const intervalId = window.setInterval(() => void checkDueSchedules(), 15_000);
    return () => {
      disposed = true;
      window.clearInterval(intervalId);
    };
  }, [hasSavedToken, delaySeconds, queueNeedsSessionRefresh]);

  useEffect(() => {
    const storedRaw = window.localStorage.getItem(SIDEBAR_WIDTH_STORAGE_KEY);
    let initialWidth = SIDEBAR_DEFAULT_WIDTH;
    if (storedRaw !== null) {
      const parsed = Number(storedRaw);
      // Sanitize: only accept finite numbers within a reasonable range
      // Extremely large values (> 10000) or non-finite values are treated as invalid
      if (Number.isFinite(parsed) && parsed > 0 && parsed <= 10000) {
        initialWidth = clampSidebarWidth(parsed);
      }
      // If invalid, keep the default SIDEBAR_DEFAULT_WIDTH
    }
    setSidebarWidth(initialWidth);
    liveSidebarWidth.current = initialWidth;
    setIsSidebarCollapsed(window.localStorage.getItem(SIDEBAR_COLLAPSED_STORAGE_KEY) === "true");
  }, []);

  useEffect(() => {
    window.localStorage.setItem(DOWNLOAD_DELAY_STORAGE_KEY, String(normalizeDelaySeconds(delaySeconds)));
  }, [delaySeconds]);

  useEffect(() => {
    const bounds = normalizeVideoWaitBounds(videoWaitMinSeconds, videoWaitMaxSeconds);
    window.localStorage.setItem(VIDEO_WAIT_MIN_STORAGE_KEY, String(bounds.minSeconds));
    window.localStorage.setItem(VIDEO_WAIT_MAX_STORAGE_KEY, String(bounds.maxSeconds));
  }, [videoWaitMinSeconds, videoWaitMaxSeconds]);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_WIDTH_STORAGE_KEY, String(sidebarWidth));
    // Sync state to live ref and CSS variable for non-drag updates (e.g., keyboard resize)
    liveSidebarWidth.current = sidebarWidth;
    shellRef.current?.style.setProperty("--sidebar-width", `${sidebarWidth}px`);
  }, [sidebarWidth]);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_COLLAPSED_STORAGE_KEY, String(isSidebarCollapsed));
  }, [isSidebarCollapsed]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    if (isTauriRuntime()) {
      void getCurrentWindow().setTheme(theme).catch(() => {
        /* Title-bar theme sync is best-effort on older WebView2 builds. */
      });
    }
  }, [theme]);

  useEffect(() => {
    const handleViewMode = (event: Event) => {
      const detail = (event as CustomEvent<ClippingViewMode>).detail;
      if (detail === "gallery" || detail === "list") setClippingViewMode(detail);
      else setClippingViewMode(readClippingViewMode());
    };
    window.addEventListener(CLIPPING_VIEW_MODE_EVENT, handleViewMode);
    return () => window.removeEventListener(CLIPPING_VIEW_MODE_EVENT, handleViewMode);
  }, []);

  useEffect(() => {
    let resizeFrame: number | null = null;
    let resizeSettledTimer: number | null = null;
    const root = document.documentElement;

    function handleWindowResize() {
      if (root.dataset.windowResizing !== "true" && resizeFrame === null) {
        resizeFrame = window.requestAnimationFrame(() => {
          resizeFrame = null;
          root.dataset.windowResizing = "true";
        });
      }
      if (resizeSettledTimer !== null) window.clearTimeout(resizeSettledTimer);
      resizeSettledTimer = window.setTimeout(() => {
        resizeSettledTimer = null;
        delete root.dataset.windowResizing;
      }, 140);
    }

    window.addEventListener("resize", handleWindowResize, { passive: true });
    return () => {
      window.removeEventListener("resize", handleWindowResize);
      if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
      if (resizeSettledTimer !== null) window.clearTimeout(resizeSettledTimer);
      delete root.dataset.windowResizing;
    };
  }, []);

  useEffect(() => {
    if (wasSettingsOpen.current && !isSettingsOpen) {
      window.requestAnimationFrame(() => {
        document.querySelector<HTMLElement>('[aria-label="Open settings"]')?.focus();
      });
    }
    wasSettingsOpen.current = isSettingsOpen;
  }, [isSettingsOpen]);

  useEffect(() => {
    if (!isSettingsOpen) return;
    const preferences = readNewspaperReaderPreferences();
    setNewspaperDefaultZoom(preferences.defaultZoom);
    setNewspaperClickZoom(preferences.clickZoom);
    setNewspaperPageTone(preferences.pageTone);
    setNewspaperOptimizationPreferences(readNewspaperOptimizationPreferences());
  }, [isSettingsOpen]);

  function startSidebarResize(event: ReactPointerEvent<HTMLButtonElement>) {
    if (isSidebarCollapsed) return;
    // Only handle primary button (left click or touch)
    if (event.button !== 0 && event.pointerType === "mouse") return;
    
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    
    // Use liveSidebarWidth as the single source of truth for drag start
    const currentWidth = liveSidebarWidth.current;
    sidebarDragStart.current = { x: event.clientX, width: currentWidth };
    sidebarDragWidth.current = currentWidth;
    setIsDraggingSidebar(true);
    sidebarDragCleanup.current?.();

    function handlePointerMove(moveEvent: PointerEvent) {
      const nextWidth = sidebarDragStart.current.width + moveEvent.clientX - sidebarDragStart.current.x;
      const clampedWidth = clampSidebarWidth(nextWidth);
      sidebarDragWidth.current = clampedWidth;
      // Update live ref immediately for single source of truth
      liveSidebarWidth.current = clampedWidth;
      if (sidebarDragAnimationFrame.current !== null) return;
      sidebarDragAnimationFrame.current = window.requestAnimationFrame(() => {
        sidebarDragAnimationFrame.current = null;
        shellRef.current?.style.setProperty("--sidebar-width", `${clampedWidth}px`);
      });
    }

    function stopDragging(commit: boolean) {
      if (sidebarDragAnimationFrame.current !== null) {
        window.cancelAnimationFrame(sidebarDragAnimationFrame.current);
        sidebarDragAnimationFrame.current = null;
      }
      const finalWidth = sidebarDragWidth.current;
      liveSidebarWidth.current = finalWidth;
      shellRef.current?.style.setProperty("--sidebar-width", `${finalWidth}px`);
      if (commit) {
        // Only sync to React state on commit - this is the single writer for persistence
        setSidebarWidth(finalWidth);
        setIsDraggingSidebar(false);
      }
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      target.removeEventListener("pointermove", handlePointerMove);
      target.removeEventListener("pointerup", finishDragging);
      target.removeEventListener("pointercancel", cancelDragging);
      target.removeEventListener("lostpointercapture", cancelDragging);
      try {
        target.releasePointerCapture(event.pointerId);
      } catch {
        // Pointer capture may already be released
      }
      sidebarDragCleanup.current = null;
    }

    function finishDragging() {
      stopDragging(true);
    }

    function cancelDragging() {
      stopDragging(false);
    }

    document.body.style.cursor = "ew-resize";
    document.body.style.userSelect = "none";
    target.addEventListener("pointermove", handlePointerMove);
    target.addEventListener("pointerup", finishDragging);
    target.addEventListener("pointercancel", cancelDragging);
    target.addEventListener("lostpointercapture", cancelDragging);
    sidebarDragCleanup.current = () => stopDragging(false);
    event.preventDefault();
  }

  useEffect(() => {
    return () => {
      sidebarDragCleanup.current?.();
      if (sidebarDragAnimationFrame.current !== null) {
        window.cancelAnimationFrame(sidebarDragAnimationFrame.current);
      }
    };
  }, []);

  async function refreshBootstrapState(): Promise<BootstrapState | null> {
    if (!isTauriRuntime()) {
      const previewPreferences = readPreviewPreferences();
      if (previewPreferences && !downloadPreferencesHydratedRef.current) {
        applyDownloadPreferences(previewPreferences, true);
        downloadPreferencesHydratedRef.current = true;
      }
      const previewState = getBrowserPreviewState();
      if (previewState) {
        setQueuedJobs(previewState.jobs);
        setRecentEvents((previous) => serializedStateEqual(previous, previewState.events) ? previous : previewState.events);
        setHasSavedToken(hasPreviewSavedToken());
        setDownloadHistory(downloadHistoryFromJobs(previewState.jobs));
        setDownloadHistoryFilePath(previewDownloadHistoryFilePath());
        return {
          default_resolution: "P720",
          browser_sources: ["Chrome", "Edge", "Firefox"],
          stores_plaintext_tokens_in_sqlite: false,
          has_saved_token: hasPreviewSavedToken(),
          saved_download_preferences: previewPreferences,
          persisted_jobs: previewState.jobs,
          recent_events: previewState.events,
          download_history: downloadHistoryFromJobs(previewState.jobs),
          download_history_file_path: previewDownloadHistoryFilePath()
        };
      }
      return null;
    }

    try {
      const state = await invoke<BootstrapState>("bootstrap_state");
      const nextBrowserSources = state.browser_sources.length > 0 ? state.browser_sources : browserSources;
      setBrowserSources((previous) => serializedStateEqual(previous, nextBrowserSources) ? previous : nextBrowserSources);
      const preferences = state.saved_download_preferences;
      if (!downloadPreferencesHydratedRef.current) {
        if (preferences) {
          applyDownloadPreferences(preferences, true);
        } else if (state.default_resolution) {
          setResolution(String(state.default_resolution).replace("P", ""));
        }
        downloadPreferencesHydratedRef.current = true;
      }

      setQueuedJobs((previous) => serializedStateEqual(previous, state.persisted_jobs) ? previous : state.persisted_jobs);
      setRecentEvents((previous) => serializedStateEqual(previous, state.recent_events) ? previous : state.recent_events);
      setHasSavedToken(state.has_saved_token);
      const nextHistory = state.download_history ?? [];
      setDownloadHistory((previous) => serializedStateEqual(previous, nextHistory) ? previous : nextHistory);
      setDownloadHistoryFilePath(state.download_history_file_path ?? "");
      return state;
    } catch {
      // Browser-only Vite previews do not expose Tauri commands.
      const previewState = getBrowserPreviewState();
      if (previewState) {
        setQueuedJobs(previewState.jobs);
        setRecentEvents((previous) => serializedStateEqual(previous, previewState.events) ? previous : previewState.events);
        setHasSavedToken(hasPreviewSavedToken());
        setDownloadHistory(downloadHistoryFromJobs(previewState.jobs));
        setDownloadHistoryFilePath(previewDownloadHistoryFilePath());
        return {
          default_resolution: "P720",
          browser_sources: ["Chrome", "Edge", "Firefox"],
          stores_plaintext_tokens_in_sqlite: false,
          has_saved_token: hasPreviewSavedToken(),
          saved_download_preferences: readPreviewPreferences(),
          persisted_jobs: previewState.jobs,
          recent_events: previewState.events,
          download_history: downloadHistoryFromJobs(previewState.jobs),
          download_history_file_path: previewDownloadHistoryFilePath()
        };
      }
      return null;
    }
  }

  function markNextQueuedJobActiveForLiveStats() {
    setQueuedJobs((jobs) => {
      const queuedIndex = jobs.findIndex((job) => job.status === "queued" && !job.paused);
      if (queuedIndex < 0) return jobs;
      return jobs.map((job, index) => (
        index === queuedIndex
          ? { ...job, status: "active", updated_at: Math.floor(Date.now() / 1000) }
          : job
      ));
    });
  }

  function currentDownloadPreferences(): SavedDownloadPreferences {
    const videoWait = normalizeVideoWaitBounds(videoWaitMinSeconds, videoWaitMaxSeconds);
    return {
      outputDir: folder,
      selectedQuality: resolution,
      delaySeconds: normalizeDelaySeconds(delaySeconds),
      videoWaitMinSeconds: videoWait.minSeconds,
      videoWaitMaxSeconds: videoWait.maxSeconds,
      browserSource,
      downloadVideos,
      downloadExercises,
      downloadSubtitles,
      downloadQuizzes
    };
  }

  function applyDownloadPreferences(preferences: SavedDownloadPreferences, preserveStoredDelay = false) {
    setFolder(preferences.outputDir);
    setResolution(preferences.selectedQuality);
    if (!preserveStoredDelay || initialStoredDelaySeconds.current === null) {
      setDelaySeconds(normalizeDelaySeconds(preferences.delaySeconds));
    }
    const videoWait = normalizeVideoWaitBounds(
      preferences.videoWaitMinSeconds ?? DEFAULT_VIDEO_WAIT_MIN_SECONDS,
      preferences.videoWaitMaxSeconds ?? DEFAULT_VIDEO_WAIT_MAX_SECONDS
    );
    setVideoWaitMinSeconds(videoWait.minSeconds);
    setVideoWaitMaxSeconds(videoWait.maxSeconds);
    setBrowserSource(preferences.browserSource);
    setDownloadVideos(preferences.downloadVideos);
    setDownloadExercises(preferences.downloadExercises);
    setDownloadSubtitles(preferences.downloadSubtitles);
    setDownloadQuizzes(preferences.downloadQuizzes ?? true);
  }

  function applyBootstrapState(state: BootstrapState) {
    if (state.saved_download_preferences) {
      applyDownloadPreferences(state.saved_download_preferences);
    }
    setQueuedJobs((previous) =>
      serializedStateEqual(previous, state.persisted_jobs) ? previous : state.persisted_jobs
    );
    setRecentEvents((previous) =>
      serializedStateEqual(previous, state.recent_events) ? previous : state.recent_events
    );
    const nextHistory = state.download_history ?? [];
    setDownloadHistory((previous) =>
      serializedStateEqual(previous, nextHistory) ? previous : nextHistory
    );
    setDownloadHistoryFilePath(state.download_history_file_path ?? "");
  }

  async function chooseLinkedInFolder(current: string): Promise<string | null> {
    if (!isTauriRuntime()) {
      guardedToast("Folder picker unavailable in preview", "The native folder picker is available in the Tauri desktop runtime.");
      return null;
    }

    try {
      const picked = await open({
        directory: true,
        multiple: false,
        defaultPath: parseDestination(current) ?? undefined
      });
      if (typeof picked !== "string" || !picked.trim()) {
        return null;
      }
      const committed = await commitLinkedInDestination<BootstrapState>(picked);
      applyBootstrapState(committed.bootstrap);
      if (committed.imported > 0) {
        toast.success(
          `Recovered ${committed.imported} LinkedIn course${committed.imported === 1 ? "" : "s"}`
        );
      } else if (committed.alreadyKnown > 0) {
        toast.info("LinkedIn folder updated", {
          description: `${committed.alreadyKnown} course${committed.alreadyKnown === 1 ? "" : "s"} already in your library.`
        });
      } else if (committed.skipped > 0) {
        toast.info("LinkedIn folder updated", {
          description: `${committed.skipped} folder${committed.skipped === 1 ? "" : "s"} skipped because they did not match a LinkedIn layout.`
        });
      } else {
        toast.success("LinkedIn download folder updated", { description: committed.outputDir });
      }
      return committed.outputDir;
    } catch (error) {
      toast.error("LinkedIn folder commit failed", { description: String(error) });
      return null;
    }
  }

  function clearDownloadEmulatorTimers() {
    for (const timerId of emulatorTimersRef.current) {
      window.clearTimeout(timerId);
    }
    emulatorTimersRef.current = [];
  }

  function stopDownloadEmulator(notify = true) {
    emulatorRunRef.current += 1;
    clearDownloadEmulatorTimers();
    setEmulatorJobs([]);
    setEmulatorEvents([]);
    if (notify) {
      toast.info("Download emulator cleared", {
        description: "The local test job was removed without touching LinkedIn or your files."
      });
    }
  }

  async function startYouTubeDownloadMock() {
    setIsSettingsOpen(false);
    try {
      await startYouTubeUiMock();
      const opened = await requestNavigation("youtube");
      if (!opened) return;
      toast.success("YouTube mock download started", {
        description: "Four fake videos are progressing in the YouTube tab. Pause and Cancel work on this local mock only."
      });
    } catch (error: unknown) {
      toast.error("YouTube mock could not start", { description: formatYouTubeInvokeError(error) });
    }
  }

  function startDownloadEmulator() {
    emulatorRunRef.current += 1;
    const runId = emulatorRunRef.current;
    clearDownloadEmulatorTimers();

    const timestamp = Math.floor(Date.now() / 1000);
    const eventBaseId = Date.now();
    const jobId = `download-emulator-${eventBaseId}`;
    const videoNames = [
      "01 - What is generative AI?",
      "02 - The attention mechanism",
      "03 - Build a responsible workflow"
    ];
    const makeVideos = (statuses: string[], updatedAt: number) => statuses.map((status, index) => ({
      ...previewVideoArtifact(
        `emulator-video-${String(index + 1).padStart(2, "0")}`,
        videoNames[index] ?? `Emulator video ${index + 1}`,
        status,
        timestamp,
        status === "completed" ? 18_000_000 + index * 3_000_000 : undefined
      ),
      updated_at: updatedAt
    }));
    const makeCounts = (
      completed: number,
      active: number,
      pending: number,
      videoCompleted: number,
      subtitleCompleted: number,
      exerciseCompleted: number
    ): ArtifactProgressCounts => ({
      total: 6,
      completed,
      failed: 0,
      cancelled: 0,
      active,
      pending,
      skipped: 0,
      video_total: 3,
      video_completed: videoCompleted,
      subtitle_total: 2,
      subtitle_completed: subtitleCompleted,
      quiz_total: 0,
      quiz_completed: 0,
      study_guide_total: 0,
      study_guide_completed: 0,
      exercise_total: 1,
      exercise_completed: exerciseCompleted
    });
    const makeEvent = (
      id: number,
      eventType: string,
      message: string,
      createdAt: number,
      payload?: Record<string, number | string>
    ): PersistedJobEvent => ({
      id,
      job_id: jobId,
      event_type: eventType,
      message,
      payload_json: payload ? JSON.stringify(payload) : null,
      created_at: createdAt
    });
    const initialJob = createDownloadEmulatorJob({
      id: jobId,
      courseSlug: "download-emulator",
      title: "Test download · Generative AI course",
      status: "active",
      timestamp
    });
    const companionJobs: QueuedDownloadJob[] = [
      createDownloadEmulatorJob({
        id: `${jobId}-queued-excel`,
        courseSlug: "excel-pivot-tables-for-analysts",
        title: "Excel: Pivot tables for analysts",
        status: "queued",
        timestamp: timestamp - 4,
        artifactCounts: makeCounts(0, 0, 6, 0, 0, 0),
        videoArtifacts: [
          previewVideoArtifact("excel-video-01", "01 - Refresh a pivot cache", "pending", timestamp - 4),
          previewVideoArtifact("excel-video-02", "02 - Calculated fields", "pending", timestamp - 4),
          previewVideoArtifact("excel-video-03", "03 - Slicers and timelines", "pending", timestamp - 4)
        ]
      }),
      createDownloadEmulatorJob({
        id: `${jobId}-queued-css`,
        courseSlug: "css-grid-and-flexbox-layouts",
        title: "CSS: Grid and flexbox layouts",
        status: "queued",
        timestamp: timestamp - 8,
        artifactCounts: makeCounts(0, 0, 6, 0, 0, 0),
        videoArtifacts: [
          previewVideoArtifact("css-video-01", "01 - Flex alignment", "pending", timestamp - 8),
          previewVideoArtifact("css-video-02", "02 - Named grid lines", "pending", timestamp - 8),
          previewVideoArtifact("css-video-03", "03 - Responsive tracks", "pending", timestamp - 8)
        ]
      }),
      createDownloadEmulatorJob({
        id: `${jobId}-paused-leadership`,
        courseSlug: "leadership-coaching-your-team",
        title: "Leadership: Coaching your team",
        status: "queued",
        paused: true,
        timestamp: timestamp - 12,
        artifactCounts: makeCounts(2, 0, 4, 1, 1, 0),
        videoArtifacts: [
          previewVideoArtifact("lead-video-01", "01 - Set a coaching cadence", "completed", timestamp - 12, 12_000_000),
          previewVideoArtifact("lead-video-02", "02 - Ask better questions", "pending", timestamp - 12),
          previewVideoArtifact("lead-video-03", "03 - Close the loop", "pending", timestamp - 12)
        ]
      })
    ];
    const publish = (job: QueuedDownloadJob, events: PersistedJobEvent[]) => {
      if (emulatorRunRef.current !== runId) return;
      setEmulatorJobs([job, ...companionJobs]);
      setEmulatorEvents(events);
    };
    const schedule = (delayMilliseconds: number, callback: () => void) => {
      const timerId = window.setTimeout(() => {
        emulatorTimersRef.current = emulatorTimersRef.current.filter((activeTimerId) => activeTimerId !== timerId);
        if (emulatorRunRef.current !== runId) return;
        callback();
      }, delayMilliseconds);
      emulatorTimersRef.current.push(timerId);
    };

    publish(initialJob, [makeEvent(eventBaseId, "job.started", "Download emulator started.", timestamp)]);

    schedule(650, () => {
      const updatedAt = Math.floor(Date.now() / 1000);
      const waitSeconds = 12;
      const waitStartedAt = updatedAt;
      const plannedJob: QueuedDownloadJob = {
        ...initialJob,
        updated_at: updatedAt,
        video_artifacts: makeVideos(["completed", "pending", "pending"], updatedAt),
        artifact_counts: makeCounts(1, 0, 5, 1, 0, 0)
      };
      publish(plannedJob, [
        makeEvent(eventBaseId + 1, "video.pacing.wait", "Test cooldown before the next video request.", updatedAt, {
          artifactId: "emulator-video-02",
          waitSeconds,
          waitStartedAt,
          waitUntil: waitStartedAt + waitSeconds
        })
      ]);
    });

    schedule(5_000, () => {
      const updatedAt = Math.floor(Date.now() / 1000);
      const downloadingJob: QueuedDownloadJob = {
        ...initialJob,
        updated_at: updatedAt,
        video_artifacts: makeVideos(["completed", "active", "pending"], updatedAt),
        artifact_counts: makeCounts(1, 1, 4, 1, 0, 0)
      };
      publish(downloadingJob, [makeEvent(eventBaseId + 2, "video.started", "Test video download started.", updatedAt)]);
    });

    schedule(9_000, () => {
      const updatedAt = Math.floor(Date.now() / 1000);
      const waitSeconds = 8;
      const waitStartedAt = updatedAt;
      const secondCooldownJob: QueuedDownloadJob = {
        ...initialJob,
        updated_at: updatedAt,
        video_artifacts: makeVideos(["completed", "completed", "pending"], updatedAt),
        artifact_counts: makeCounts(2, 0, 4, 2, 0, 0)
      };
      publish(secondCooldownJob, [
        makeEvent(eventBaseId + 3, "video.pacing.wait", "Test cooldown before the final video request.", updatedAt, {
          artifactId: "emulator-video-03",
          waitSeconds,
          waitStartedAt,
          waitUntil: waitStartedAt + waitSeconds
        })
      ]);
    });

    schedule(15_000, () => {
      const updatedAt = Math.floor(Date.now() / 1000);
      const completedJob: QueuedDownloadJob = {
        ...initialJob,
        status: "completed",
        updated_at: updatedAt,
        video_artifacts: makeVideos(["completed", "completed", "completed"], updatedAt),
        artifact_counts: makeCounts(6, 0, 0, 3, 2, 1)
      };
      publish(completedJob, [makeEvent(eventBaseId + 4, "job.completed", "Download emulator finished.", updatedAt)]);
      toast.success("Download emulator complete", {
        description: "The fake course is now available in the Completed tab. Queued mocks stay on Queue."
      });
    });

    setQueueSection("queue");
    setIsSettingsOpen(false);
    void requestNavigation("downloads");
    toast.success("Download emulator started", {
      description: "Four local mock courses; no LinkedIn requests or files are used. Click the downloading row to preview the overlay."
    });
  }

  const canStart = useMemo(
    () => courseUrls.trim().length > 0 && !isQueueingDownload,
    [courseUrls, isQueueingDownload]
  );

  const syncLinkedInUrlHeight = useCallback(() => {
    const el = courseUrlsInputRef.current;
    if (!el) return;
    el.style.height = "0px";
    const next = Math.min(
      Math.max(el.scrollHeight, LINKEDIN_URL_MIN_HEIGHT_PX),
      LINKEDIN_URL_MAX_HEIGHT_PX
    );
    el.style.height = `${next}px`;
  }, []);

  useLayoutEffect(() => {
    syncLinkedInUrlHeight();
  }, [courseUrls, syncLinkedInUrlHeight]);

  const allQueueJobs = useMemo(() => [...queuedJobs, ...emulatorJobs], [queuedJobs, emulatorJobs]);
  const allRecentEvents = useMemo(() => [...recentEvents, ...emulatorEvents], [recentEvents, emulatorEvents]);

  const liveQueueJobs = allQueueJobs.filter((job) => shouldShowInLiveQueue(job.status));
  const completedJobs = completedCourseJobs(allQueueJobs);
  const activeJobs = jobsForActivityFilter(allQueueJobs, "active");
  const failedJobs = jobsForActivityFilter(allQueueJobs, "failed");
  const displayedQueueJobs = liveQueueJobs;
  const queueSectionCount = displayedQueueJobs.length > 0 ? displayedQueueJobs.length : parsedCourses.length;
  const pausableQueueJobs = liveQueueJobs.filter((job) =>
    !isDownloadEmulatorJob(job) && (job.status === "active" || job.status === "queued")
  );
  const activeDownloadJob = pausableQueueJobs.find((job) => job.status === "active") ?? null;
  const allPausableJobsPaused = pausableQueueJobs.length > 0 && pausableQueueJobs.every((job) => job.paused);

  const activitySummary = {
    active: activeJobs.length,
    completed: completedJobs.length,
    failed: failedJobs.length
  };

  async function clearFailedQueueItems() {
    if (activitySummary.failed === 0) return;
    try {
      const state = await clearFailedDownloadJobs();
      setQueuedJobs(state.persisted_jobs);
      setHasSavedToken(state.has_saved_token);
      toast.info("Failed queue cleared", {
        description: "Failed and cancelled items were removed from the queue."
      });
    } catch (error) {
      toast.error("Clear failed queue failed", { description: String(error) });
    }
  }

  function updateDelaySeconds(value: string) {
    setDelaySeconds(normalizeDelaySeconds(value));
  }

  function updateVideoWaitBounds(nextMin: string | number, nextMax: string | number) {
    const bounds = normalizeVideoWaitBounds(nextMin, nextMax);
    setVideoWaitMinSeconds(bounds.minSeconds);
    setVideoWaitMaxSeconds(bounds.maxSeconds);
    if (!isTauriRuntime()) return;
    void invoke<[number, number]>("set_linkedin_video_wait_bounds", {
      minSeconds: bounds.minSeconds,
      maxSeconds: bounds.maxSeconds
    }).catch(() => {
      /* Live pacing update is best-effort; next save/start also refreshes bounds. */
    });
  }

  async function removeQueueItem(job: QueuedDownloadJob) {
    if (isDownloadEmulatorJob(job)) {
      stopDownloadEmulator();
      return;
    }

    if (job.status === "active") {
      const shouldCancelAndRemove = window.confirm(
        `Cancel and remove ${courseDisplayName(job)} from the download queue? In-progress work will stop at the next safe boundary.`
      );
      if (!shouldCancelAndRemove) return;
      try {
        const state = await removeDownloadQueueItem(job.id);
        setQueuedJobs(state.persisted_jobs);
        setHasSavedToken(state.has_saved_token);
        toast.info("Active download cancelled", {
          description: courseDisplayName(job)
        });
      } catch (error) {
        toast.error("Could not remove active download", { description: String(error) });
      }
      return;
    }

    const shouldRemove = window.confirm(`Remove ${courseDisplayName(job)} from the download queue?`);
    if (!shouldRemove) return;

    try {
      const state = await removeDownloadQueueItem(job.id);
      setQueuedJobs(state.persisted_jobs);
      setHasSavedToken(state.has_saved_token);
      toast.info("Queue item removed", {
        description: courseDisplayName(job)
      });
    } catch (error) {
      toast.error("Remove queue item failed", { description: String(error) });
    }
  }

  async function copyQueuedCourseUrl(job: QueuedDownloadJob) {
    const url = job.source_url.trim() || `https://www.linkedin.com/learning/${job.course_slug}`;
    try {
      await copyTextToClipboard(url);
      toast.success("Course URL copied", { description: url });
    } catch (error) {
      toast.error("Could not copy course URL", { description: String(error) });
    }
  }

  async function downloadScheduledNow(job: QueuedDownloadJob) {
    if (!isScheduledJob(job)) return;
    try {
      const state = await downloadScheduledJobNow(job.id);
      setQueuedJobs(state.persisted_jobs);
      setDownloadHistory(state.download_history ?? []);
      toast.info("Moved to immediate queue", { description: courseDisplayName(job) });
      ensureDownloadProcessing(state.has_saved_token);
    } catch (error) {
      toast.error("Could not start scheduled course", { description: String(error) });
    }
  }

  function handleTokenGuideOpenChange(open: boolean) {
    setIsTokenGuideOpen(open);
    if (!open) {
      window.localStorage.setItem(TOKEN_GUIDE_DISMISSED_STORAGE_KEY, "true");
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

  async function openScheduleDialog() {
    const parsed = parsedCourses.length > 0 ? parsedCourses : await validateUrls();
    if (parsed.length === 0) return;
    setScheduleCourseCount(parsed.length);
    setScheduleStep("configure");
    setIsScheduleOpen(true);
  }

  async function reviewDownloadSchedule() {
    const hasValidHours = Number.isInteger(scheduleWindowHours) && scheduleWindowHours >= 0 && scheduleWindowHours <= 168;
    const hasValidMinutes = Number.isInteger(scheduleWindowMinutes) && scheduleWindowMinutes >= 0 && scheduleWindowMinutes <= 59;
    if (!hasValidHours || !hasValidMinutes || scheduleWindowTotalMinutes < 1 || scheduleWindowTotalMinutes > 10_080) {
      toast.warning("Choose a valid schedule window", { description: "Use 1 minute to 7 days, with minutes between 0 and 59." });
      return;
    }
    if (scheduleMinWaitMinutes < 1 || scheduleMaxWaitMinutes < scheduleMinWaitMinutes) {
      toast.warning("Choose a valid random wait", { description: "The maximum wait must be greater than or equal to the minimum wait." });
      return;
    }
    const parsed = await validateUrls();
    if (parsed.length === 0) return;
    if (scheduleMinWaitMinutes * parsed.length > scheduleWindowTotalMinutes) {
      toast.warning("Schedule window is too short", {
        description: `At least ${scheduleMinWaitMinutes * parsed.length} minutes are needed for ${parsed.length} courses at this minimum wait.`
      });
      return;
    }
    setScheduleCourseCount(parsed.length);
    setScheduleStep("confirm");
  }

  async function queueDownloads(schedule?: DownloadScheduleRequest) {
    if (queueSubmissionRef.current) return;
    queueSubmissionRef.current = true;
    setIsQueueingDownload(true);

    try {
      const addingToActiveQueue = Boolean(downloadProcessingPromiseRef.current) || isProcessingDownload;
      const parsed = await validateUrls();
      if (parsed.length === 0) return;
      const outputDir = await ensureDestination({
        current: folder,
        ask: () => chooseLinkedInFolder(folder)
      });
      if (!outputDir) {
        document.querySelector<HTMLElement>('[aria-label="LinkedIn folder"]')?.focus();
        return;
      }
      const enteredToken = token.trim();
      let shouldUseSavedToken = Boolean(hasSavedToken);
      if (enteredToken) {
        setIsValidatingToken(true);
        try {
          await saveLinkedInToken(enteredToken);
          setHasSavedToken(true);
          shouldUseSavedToken = true;
          setToken("");
          setQueueNeedsSessionRefresh(false);
        } catch (error) {
          toast.error("Token validation failed", { description: String(error) });
          return;
        } finally {
          setIsValidatingToken(false);
        }
      } else if (schedule && !shouldUseSavedToken) {
        toast.warning("Saved session required", {
          description: "Paste and save your LinkedIn token before confirming an automatic schedule."
        });
        return;
      } else if (!shouldUseSavedToken) {
        toast.info("Using browser session", {
          description: `LinkedVault will read the ${browserSource} LinkedIn session for this download.`
        });
      }
      const completedSlugs = new Set(downloadHistory.map((entry) => entry.course_slug));
      const alreadyDownloaded = parsed
        .map((course) => course.slug)
        .filter((slug) => completedSlugs.has(slug));
      if (alreadyDownloaded.length > 0) {
        const shouldDownloadAgain = window.confirm(
          `LinkedVault has already completed ${alreadyDownloaded.length} selected LinkedIn course${alreadyDownloaded.length === 1 ? "" : "s"}:\n\n${alreadyDownloaded.join("\n")}\n\nDownload ${alreadyDownloaded.length === 1 ? "it" : "them"} again?`
        );
        if (!shouldDownloadAgain) return;
      }

      const response = await startDownloadJobs({
        courseUrls,
        outputDir,
        selectedQuality: resolution,
        delaySeconds: normalizeDelaySeconds(delaySeconds),
        videoWaitMinSeconds: normalizeVideoWaitBounds(videoWaitMinSeconds, videoWaitMaxSeconds).minSeconds,
        videoWaitMaxSeconds: normalizeVideoWaitBounds(videoWaitMinSeconds, videoWaitMaxSeconds).maxSeconds,
        browserSource,
        downloadVideos,
        downloadExercises,
        downloadSubtitles,
        downloadQuizzes,
        schedule
      });
      setQueuedJobs((jobs) => mergeQueuedJobs(jobs, response.jobs));
      setCourseUrls("");
      setParsedCourses([]);
      await refreshBootstrapState();
      if (schedule) {
        setIsScheduleOpen(false);
        setScheduleStep("configure");
        toast.success("Courses scheduled", {
          description: `${response.jobs.length} course${response.jobs.length === 1 ? "" : "s"} will start automatically over the next ${formatScheduleDuration(schedule.windowMinutes)}.`
        });
      } else {
        toast.success(addingToActiveQueue ? "Added to download queue" : "Download queued", {
          description: `${response.jobs.length} LinkedIn course${response.jobs.length === 1 ? "" : "s"} ${addingToActiveQueue ? "added behind the active download" : "persisted to the local queue"}.`
        });
        ensureDownloadProcessing(shouldUseSavedToken);
      }
    } catch (error) {
      await refreshBootstrapState();
      toast.error("Could not add download", { description: String(error) });
    } finally {
      queueSubmissionRef.current = false;
      setIsQueueingDownload(false);
    }
  }

  async function startDownload() {
    await queueDownloads();
  }

  async function confirmDownloadSchedule() {
    await queueDownloads({
      windowMinutes: scheduleWindowTotalMinutes,
      minWaitMinutes: scheduleMinWaitMinutes,
      maxWaitMinutes: scheduleMaxWaitMinutes
    });
  }

  function ensureDownloadProcessing(useSavedToken: boolean) {
    if (downloadProcessingPromiseRef.current) return;

    cancellationRequestedRef.current = false;
    setIsProcessingDownload(true);
    let processingFailed = false;
    const processPromise = processQueuedDownloadBatchWithLiveRefresh(normalizeDelaySeconds(delaySeconds), useSavedToken);
    downloadProcessingPromiseRef.current = processPromise;

    void processPromise
      .then((processResponse) => {
        setQueueNeedsSessionRefresh(false);
        if (processResponse.processed) {
          showProcessedDownloadToast(processResponse);
        } else {
          toast.info("No queued download to process", {
            description: "The local queue did not contain a pending LinkedIn course."
          });
        }
      })
      .catch(async (error) => {
        processingFailed = true;
        await refreshBootstrapState();
        if (isLinkedInSessionError(error)) {
          setQueueNeedsSessionRefresh(true);
          toast.warning("LinkedIn session needs refreshing", {
            description: "Queued courses are still safe. Paste a fresh li_at cookie above, then choose Resume queue."
          });
        } else {
          toast.error("Download processing failed", { description: String(error) });
        }
      })
      .finally(async () => {
        if (downloadProcessingPromiseRef.current === processPromise) {
          downloadProcessingPromiseRef.current = null;
        }

        const state = await refreshBootstrapState();
        const hasQueuedCourses = state ? hasReadyQueuedJobs(state.persisted_jobs) : false;
        if (!processingFailed && !cancellationRequestedRef.current && hasQueuedCourses) {
          ensureDownloadProcessing(useSavedToken);
          return;
        }
        setIsProcessingDownload(false);
    });
  }

  async function resumeQueuedDownloads() {
    if (downloadProcessingPromiseRef.current) return;

    const enteredToken = token.trim();
    let shouldUseSavedToken = Boolean(hasSavedToken);

    try {
      if (enteredToken) {
        setIsValidatingToken(true);
        await saveLinkedInToken(enteredToken);
        setHasSavedToken(true);
        setToken("");
        shouldUseSavedToken = true;
      } else if (!shouldUseSavedToken && !isTauriRuntime()) {
        toast.info("LinkedIn session required", {
          description: "Paste a fresh li_at cookie before resuming the queued courses."
        });
        return;
      }

      setQueueNeedsSessionRefresh(false);
      cancellationRequestedRef.current = false;
      ensureDownloadProcessing(shouldUseSavedToken);
    } catch (error) {
      setQueueNeedsSessionRefresh(true);
      toast.error("Session refresh failed", { description: String(error) });
    } finally {
      setIsValidatingToken(false);
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
      await sleep(400);
      if (!settled) {
        await refreshBootstrapState();
      }
    }

    const response = await processPromise;
    await refreshBootstrapState();
    return response;
  }

  async function waitForLinkedInQueueIdle(): Promise<ProcessQueuedDownloadResponse> {
    let summary = emptyProcessQueuedDownloadResponse();
    for (let i = 0; i < 120; i += 1) {
      if (cancellationRequestedRef.current) {
        return summary;
      }
      const state = await refreshBootstrapState();
      const busy = Boolean(
        state &&
          (hasReadyQueuedJobs(state.persisted_jobs) ||
            state.persisted_jobs.some((job) => job.status.toLowerCase() === "active"))
      );
      if (!busy) {
        summary.processed = true;
        return summary;
      }
      await sleep(500);
    }
    return summary;
  }

  async function processQueuedDownloadBatchWithLiveRefresh(courseDelaySeconds: number, useSavedToken: boolean) {
    if (isTauriRuntime() && useSavedToken) {
      return waitForLinkedInQueueIdle();
    }

    let summary = emptyProcessQueuedDownloadResponse();

    while (!cancellationRequestedRef.current) {
      const response = await processQueuedDownloadWithLiveRefresh(() =>
        useSavedToken ? processNextQueuedDownloadWithSavedToken() : processNextQueuedDownloadWithBrowserSource(browserSource)
      );
      summary = mergeProcessQueuedDownloadResponses(summary, response);

      const state = await refreshBootstrapState();
      if (!response.processed || response.cancelled_artifacts > 0 || cancellationRequestedRef.current) {
        return summary;
      }

      const hasRemainingQueuedJobs = state ? hasReadyQueuedJobs(state.persisted_jobs) : false;
      if (!hasRemainingQueuedJobs) {
        return summary;
      }

      const delayMs = Math.max(0, courseDelaySeconds) * 1000;
      if (delayMs > 0) {
        await sleepUntilNextQueueItem(delayMs, () => cancellationRequestedRef.current);
      }
    }

    return summary;
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
    if (!parseDestination(folder)) {
      writeNewspaperReaderPreferences({
        defaultZoom: newspaperDefaultZoom,
        clickZoom: newspaperClickZoom,
        pageTone: newspaperPageTone
      });
      toast.success("Newspaper settings saved", {
        description: "Choose a LinkedIn download folder before saving downloader defaults."
      });
      return;
    }

    setIsSavingSettings(true);
    const startedAt = Date.now();
    try {
      const preferences = await saveDownloadPreferences(currentDownloadPreferences());
      applyDownloadPreferences(preferences);
      writeNewspaperReaderPreferences({
        defaultZoom: newspaperDefaultZoom,
        clickZoom: newspaperClickZoom,
        pageTone: newspaperPageTone
      });
      toast.success("Settings saved", {
        description: "Download defaults will be restored the next time LinkedVault opens."
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

  async function registerNewspaperArchive() {
    if (!isTauriRuntime()) return;
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Recover newspaper archive"
    });
    if (typeof picked !== "string") return;
    setIsRegisteringNewspaperArchive(true);
    try {
      const imported = await invoke<number>("import_existing_newspaper_archive", { path: picked });
      toast.success(`Recovered ${imported} newspaper edition${imported === 1 ? "" : "s"}.`);
    } catch (error) {
      toast.error("Could not recover newspaper archive", { description: String(error) });
    } finally {
      setIsRegisteringNewspaperArchive(false);
    }
  }

  async function repairNewspaperLibrary() {
    if (!isTauriRuntime()) return;
    setIsRepairingNewspaperLibrary(true);
    try {
      const result = await invoke<{
        renamedFiles: number;
        optimizedJobs: number;
        removedSourceFiles: number;
        warnings: string[];
      }>("repair_newspaper_library");
      toast.success("Newspaper library repair finished.", {
        description: `${result.optimizedJobs} optimized, ${result.renamedFiles} renamed, ${result.removedSourceFiles} redundant source JPGs removed.${result.warnings.length ? ` ${result.warnings.length} warning(s).` : ""}`
      });
    } catch (error) {
      toast.error("Could not repair newspaper library", { description: String(error) });
    } finally {
      setIsRepairingNewspaperLibrary(false);
    }
  }

  async function checkForUpdates() {
    setIsCheckingUpdate(true);
    try {
      const update = await checkForAppUpdate();
      setPendingUpdate(update);
      if (update) {
        setUpdateBannerDismissed(false);
        return;
      }
      toast.info("LinkedVault is up to date", {
        description: `Current version ${APP_VERSION} is installed.`
      });
    } catch (error) {
      toast.error("Update check failed", { description: String(error) });
    } finally {
      setIsCheckingUpdate(false);
    }
  }

  async function checkForUpdatesOnLaunch() {
    if (!isTauriRuntime()) return;

    try {
      const update = await checkForAppUpdate();
      setPendingUpdate(update);
      // Reset dismissed state so the banner re-appears if a new update is available.
      if (update) setUpdateBannerDismissed(false);
    } catch {
      // Startup checks should never block downloading courses.
    }
  }

  async function installUpdate(updateToInstall: UpdateMetadata | null = pendingUpdate) {
    if (!updateToInstall) {
      toast.warning("No update selected", {
        description: "Check for updates before installing."
      });
      return;
    }
    if (clippingFlushRef.current && !(await clippingFlushRef.current())) {
      toast.error("Update paused", {
        description: "Save or resolve the current clipping note before installing an update."
      });
      return;
    }

    setIsInstallingUpdate(true);
    try {
      await installAppUpdate();
      toast.dismiss(UPDATE_TOAST_ID);
      setUpdateBannerDismissed(true);
      toast.success("Update installed", {
        description: "Restart LinkedVault to finish using the new version."
      });
    } catch (error) {
      toast.error("Update install failed", { description: String(error) });
    } finally {
      setIsInstallingUpdate(false);
    }
  }

  useEffect(() => {
    if (!pendingUpdate || updateBannerDismissed) {
      if (updateBannerDismissed) toast.dismiss(UPDATE_TOAST_ID);
      return;
    }

    const version = pendingUpdate.version;
    toast.custom((toastId) => (
      <div className="lv-toast lv-toast-update" role="status">
        <div className="lv-toast-copy">
          <strong className="lv-toast-title">Update available</strong>
          <span className="lv-toast-description">LinkedVault {version} is ready to install.</span>
        </div>
        <div className="lv-toast-update-actions">
          <button
            type="button"
            className="lv-toast-update-install"
            disabled={isInstallingUpdate}
            onClick={() => {
              void installUpdate(pendingUpdate);
            }}
          >
            {isInstallingUpdate ? "Installing" : "Install now"}
          </button>
          <button
            type="button"
            className="lv-toast-update-close"
            aria-label="Dismiss update"
            onClick={() => {
              toast.dismiss(toastId);
              setUpdateBannerDismissed(true);
            }}
          >
            <X aria-hidden="true" className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
    ), {
      id: UPDATE_TOAST_ID,
      duration: Infinity
    });
  }, [isInstallingUpdate, pendingUpdate, updateBannerDismissed]);

  async function cancelDownload() {
    if (!activeDownloadJob) return;
    cancellationRequestedRef.current = true;
    setIsCancellingDownload(true);
    try {
      const response = await requestActiveDownloadCancellation();
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

  async function toggleDownloadPause(job: QueuedDownloadJob) {
    if (job.status !== "active" && job.status !== "queued") return;
    const nextPaused = !job.paused;
    setPauseUpdatingTaskId(job.id);
    try {
      const state = await setDownloadJobPause(job.id, nextPaused);
      setQueuedJobs(state.persisted_jobs);
      toast.info(nextPaused ? "Download paused" : "Download resumed", {
        description: nextPaused
          ? `${courseDisplayName(job)} will pause at the next safe boundary.`
          : `${courseDisplayName(job)} is available to continue.`
      });
      if (!nextPaused && (job.status === "queued" || job.status === "active") && (isTauriRuntime() || hasSavedToken)) {
        cancellationRequestedRef.current = false;
        ensureDownloadProcessing(hasSavedToken);
      }
    } catch (error) {
      toast.error(nextPaused ? "Pause failed" : "Resume failed", { description: String(error) });
    } finally {
      setPauseUpdatingTaskId(null);
    }
  }

  async function toggleAllDownloadsPause() {
    if (pausableQueueJobs.length === 0) return;
    const nextPaused = !allPausableJobsPaused;
    setIsPausingAll(true);
    try {
      const state = await setAllDownloadsPaused(nextPaused);
      setQueuedJobs(state.persisted_jobs);
      toast.info(nextPaused ? "All downloads paused" : "All downloads resumed", {
        description: nextPaused
          ? "Active work will pause at the next safe boundary. Queued and scheduled courses will wait."
          : "Queued downloads are available to continue."
      });
      if (!nextPaused && (isTauriRuntime() || hasSavedToken)) {
        cancellationRequestedRef.current = false;
        ensureDownloadProcessing(hasSavedToken);
      }
    } catch (error) {
      toast.error(nextPaused ? "Pause all failed" : "Resume all failed", { description: String(error) });
    } finally {
      setIsPausingAll(false);
    }
  }

  // The reset-data flow lives in three pieces: the request handler that
  // opens a confirmation dialog, the per-provider confirmation handler that
  // performs the auto-pause + wipe, and the small derived state above the
  // Settings panel that drives the disabled state on the trigger buttons.

  type ResetProvider = "linkedin" | "coursera" | "newspaper";

  function activeLinkedinJobCount(): number {
    return queuedJobs.filter((job) => job.status === "active" && !job.paused).length;
  }

  async function requestResetProvider(provider: ResetProvider) {
    if (resetInProgress || pausingForReset || pendingResetProvider) return;
    if (clippingFlushRef.current && !(await clippingFlushRef.current())) {
      toast.error("Reset paused", {
        description: "Your clipping draft must be saved or resolved before resetting provider data."
      });
      return;
    }
    setPendingResetProvider(provider);
  }

  async function performProviderReset(provider: ResetProvider) {
    if (!isTauriRuntime()) {
      toast.info("Browser preview", { description: "Run the Tauri app to reset provider data." });
      setPendingResetProvider(null);
      return;
    }
    setPendingResetProvider(null);
    setResetInProgress(provider);
    try {
      // Step 1: auto-pause the in-flight worker via the existing
      // bulk-pause command. The worker unwinds at the next safe boundary;
      // give it a short grace window before the wipe commits.
      const hasActiveWork = await pauseProviderForReset(provider);
      if (hasActiveWork) {
        await new Promise((resolve) => window.setTimeout(resolve, 1_500));
      }
      // Step 2: snapshot the output folders the user might want to clean up
      // on disk. This is best-effort — the wipe itself does not touch them.
      const outputDirs = provider === "newspaper" ? collectNewspaperOutputDirs() : [];
      // Step 3: run the provider-specific wipe.
      const counts = await invokeProviderReset(provider);
      const cleared = describeProviderCounts(provider, counts);
      if (provider === "linkedin") {
        await refreshBootstrapState();
      }
      toast.success(provider === "newspaper"
        ? "World Journal download data was reset"
        : `${resetProviderLabel(provider)} database cleared`, {
        description: provider === "newspaper"
          ? `Your saved clippings and notes were preserved. ${cleared}`
          : cleared,
        action: outputDirs.length > 0
          ? {
              label: "Open output folder",
              onClick: () => {
                const next = outputDirs[0];
                void invoke("open_newspaper_download_folder", { path: next }).catch(() => undefined);
              }
            }
          : undefined,
      });
    } catch (error) {
      toast.error(`Could not reset ${resetProviderLabel(provider)} database`, {
        description: String(error)
      });
    } finally {
      setResetInProgress(null);
      setPausingForReset(null);
    }
  }

  async function pauseProviderForReset(provider: ResetProvider): Promise<boolean> {
    setPausingForReset(provider);
    try {
      if (provider === "linkedin") {
        if (pausableQueueJobs.length === 0) return false;
        const state = await setAllDownloadsPaused(true);
        setQueuedJobs(state.persisted_jobs);
        return true;
      }
      if (provider === "coursera") {
        // Coursera has no per-job pause UI; the existing
        // cancel_active_coursera_download command sets the cooperative
        // flag and the worker unwinds at a safe boundary. The defensive
        // re-arm in reset_coursera_database handles the case where the
        // worker has already exited.
        const cancelled = await invoke<boolean>("cancel_active_coursera_download");
        if (!cancelled) {
          // Either there was no active worker or the call returned false;
          // either way we proceed with the wipe.
        }
        return true;
      }
      const updated = await invoke<string[]>("set_all_newspaper_jobs_paused", {
        paused: true
      });
      return updated.length > 0;
    } catch (error) {
      // The pause step is best-effort. The reset command will defensively
      // re-arm the cancellation flag, so we still proceed to the wipe.
      toast.warning(`Could not pause ${resetProviderLabel(provider)} downloads first`, {
        description: String(error)
      });
      return false;
    }
  }

  async function invokeProviderReset(provider: ResetProvider): Promise<Record<string, number>> {
    if (provider === "linkedin") {
      return await invoke<Record<string, number>>("reset_linkedin_database");
    }
    if (provider === "coursera") {
      return await invoke<Record<string, number>>("reset_coursera_database");
    }
    return await invoke<Record<string, number>>("reset_newspaper_database");
  }

  function resetProviderLabel(provider: ResetProvider): string {
    return provider === "linkedin"
      ? "LinkedIn"
      : provider === "coursera"
        ? "Coursera"
        : "World Journal";
  }

  function describeProviderCounts(provider: ResetProvider, counts: Record<string, number>): string {
    const parts: string[] = [];
    for (const [key, value] of Object.entries(counts)) {
      if (!value) continue;
      const label = key.replace(/_/g, " ");
      parts.push(`${value} ${label}`);
    }
    if (parts.length === 0) {
      return "The selected tables were already empty.";
    }
    return `Cleared ${parts.join(", ")} from the ${resetProviderLabel(provider)} tables.`;
  }

  function collectNewspaperOutputDirs(): string[] {
    // Output folders are tracked per batch in the bootstrap state. Without a
    // dedicated command, fall back to the current NewspaperView state via
    // the global poll. Returning an empty array here means the
    // open-folder action simply does not appear, which is safer than
    // guessing.
    return [];
  }

  async function retryDownloadJob(job: QueuedDownloadJob) {
    if (job.status !== "failed") return;
    const enteredToken = token.trim();
    let shouldUseSavedToken = Boolean(hasSavedToken);

    try {
      setIsProcessingDownload(true);
      if (enteredToken) {
        await saveLinkedInToken(enteredToken);
        setHasSavedToken(true);
        shouldUseSavedToken = true;
        setToken("");
        setQueueNeedsSessionRefresh(false);
      } else if (!shouldUseSavedToken) {
        toast.info("Using browser session", {
          description: `LinkedVault will read the ${browserSource} LinkedIn session for this retry.`
        });
      }
      await retryFailedDownloadJob(job.id);
      setQueuedJobs((jobs) =>
        jobs.map((candidate) =>
          candidate.id === job.id
            ? { ...candidate, status: "queued", paused: false, artifact_counts: emptyArtifactCounts() }
            : candidate
        )
      );
      toast.info("Retry queued", { description: courseDisplayName(job) });

      cancellationRequestedRef.current = false;
      const processResponse = await processQueuedDownloadBatchWithLiveRefresh(delaySeconds, shouldUseSavedToken);

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
      if (isLinkedInSessionError(error)) {
        setQueueNeedsSessionRefresh(true);
        toast.warning("LinkedIn session needs refreshing", {
          description: "The failed course was kept available for retry. Paste a fresh li_at cookie above, then retry it."
        });
      } else {
        toast.error("Retry failed", { description: String(error) });
      }
    } finally {
      setIsProcessingDownload(false);
    }
  }

  async function openCompletedFolder(job: QueuedDownloadJob) {
    if (isDownloadEmulatorJob(job)) {
      toast.info("Test download has no files", {
        description: "The emulator only exercises queue state and never writes to disk."
      });
      return;
    }
    await openCompletedFolderByJobId(job.id, job.output_dir);
  }

  async function openCompletedFolderByJobId(jobId: string, fallbackPath?: string) {
    if (!jobId.trim() && !fallbackPath?.trim()) {
      toast.warning("Folder unavailable", { description: "This completed course does not have a saved output folder." });
      return;
    }

    try {
      const opened = await openDownloadFolder(jobId, fallbackPath);
      if (opened) {
        toast.success("Folder opened", { description: opened.path });
      }
    } catch (error) {
      toast.error("Open folder failed", { description: String(error) });
    }
  }

  async function browseDownloadFolder(): Promise<string | null> {
    return chooseLinkedInFolder(folder);
  }

  return (
    <>
    <div
      ref={shellRef}
      className="lv-shell"
      data-sidebar-dragging={isDraggingSidebar || undefined}
      data-sidebar-state={isSidebarCollapsed ? "collapsed" : "expanded"}
    >
      <aside className="lv-sidebar" aria-label="Primary navigation">
        <div className="lv-sidebar-trigger-wrap">
          <Tooltip label="Toggle sidebar">
            <IconButton className="lv-sidebar-trigger" aria-label="Toggle sidebar" aria-expanded={!isSidebarCollapsed} onClick={() => setIsSidebarCollapsed(true)}>
              <PanelLeft aria-hidden="true" className="h-4 w-4" />
            </IconButton>
          </Tooltip>
        </div>
        <div className="lv-sidebar-brand border-b border-sidebar-border">
          <div className="lv-brand-logo" aria-label="LinkedVault">
            <span className="lv-brand-wordmark">LinkedVault</span>
          </div>
          <h1 className="sr-only">LinkedVault</h1>
        </div>

        <nav className="grid flex-1 content-start gap-1 px-3 py-3 text-xs">
          <div className="lv-nav-group">
            <SidebarItem
              icon={<IconBrandLinkedin aria-hidden="true" size={18} />}
              trailing={<ChevronDown aria-hidden="true" className="lv-nav-chevron" />}
              aria-expanded={isLinkedInExpanded}
              onClick={() => {
                const isCurrentProvider = activeView === "downloads" || activeView === "linkedin-history";
                if (isCurrentProvider) {
                  setIsLinkedInExpanded((expanded) => {
                    const nextExpanded = !expanded;
                    if (nextExpanded) void requestNavigation("downloads");
                    return nextExpanded;
                  });
                } else {
                  void requestNavigation("downloads");
                  setIsLinkedInExpanded(true);
                }
              }}
            >
              LinkedIn Courses
            </SidebarItem>
            <div className="lv-nav-children" hidden={!isLinkedInExpanded}>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "downloads"}
                icon={<Download aria-hidden="true" />}
                aria-label="Download LinkedIn courses"
                onClick={() => void requestNavigation("downloads")}
              >
                Download LinkedIn
              </SidebarItem>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "linkedin-history"}
                icon={<History aria-hidden="true" />}
                aria-label="LinkedIn download history"
                onClick={() => void requestNavigation("linkedin-history")}
              >
                Download history
              </SidebarItem>
            </div>
          </div>
          <div className="lv-nav-group">
            <SidebarItem
              icon={<IconCertificate aria-hidden="true" size={18} />}
              trailing={<ChevronDown aria-hidden="true" className="lv-nav-chevron" />}
              aria-expanded={isCourseraExpanded}
              onClick={() => {
                const isCurrentProvider = activeView === "coursera" || activeView === "coursera-history";
                if (isCurrentProvider) {
                  setIsCourseraExpanded((expanded) => {
                    const nextExpanded = !expanded;
                    if (nextExpanded) void requestNavigation("coursera");
                    return nextExpanded;
                  });
                } else {
                  void requestNavigation("coursera");
                  setIsCourseraExpanded(true);
                }
              }}
            >
              Coursera Courses
            </SidebarItem>
            <div className="lv-nav-children" hidden={!isCourseraExpanded}>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "coursera"}
                icon={<Download aria-hidden="true" />}
                aria-label="Download Coursera courses"
                onClick={() => void requestNavigation("coursera")}
              >
                Download Coursera
              </SidebarItem>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "coursera-history"}
                icon={<History aria-hidden="true" />}
                aria-label="Coursera download history"
                onClick={() => void requestNavigation("coursera-history")}
              >
                Download history
              </SidebarItem>
            </div>
          </div>
          <div className="lv-nav-group">
            <SidebarItem
              icon={<Newspaper aria-hidden="true" size={18} />}
              trailing={<ChevronDown aria-hidden="true" className="lv-nav-chevron" />}
              aria-expanded={isNewspaperExpanded}
              onClick={() => {
                const isCurrentProvider = activeView === "newspaper-download" || activeView === "newspaper-library" || activeView === "newspaper-clippings";
                if (isCurrentProvider) {
                  setIsNewspaperExpanded((expanded) => {
                    const nextExpanded = !expanded;
                    if (nextExpanded) void requestNavigation("newspaper-download");
                    return nextExpanded;
                  });
                } else {
                  void requestNavigation("newspaper-download");
                  setIsNewspaperExpanded(true);
                }
              }}
            >
              World Journal
            </SidebarItem>
            <div className="lv-nav-children" hidden={!isNewspaperExpanded}>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "newspaper-download"}
                icon={<Download aria-hidden="true" />}
                onClick={() => void requestNavigation("newspaper-download")}
              >
                Download editions
              </SidebarItem>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "newspaper-library"}
                icon={<History aria-hidden="true" />}
                onClick={() => void requestNavigation("newspaper-library")}
              >
                Newspaper library
              </SidebarItem>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "newspaper-clippings"}
                icon={<StickyNote aria-hidden="true" />}
                onClick={() => void clippingNavigation.openGallery()}
              >
                Clippings
              </SidebarItem>
            </div>
          </div>
          <div className="lv-nav-group">
            <SidebarItem
              icon={<IconMovie aria-hidden="true" size={18} />}
              trailing={<ChevronDown aria-hidden="true" className="lv-nav-chevron" />}
              aria-expanded={isYouTubeExpanded}
              aria-label="Open YouTube archive"
              onClick={() => {
                const isCurrentProvider = activeView === "youtube" || activeView === "youtube-history";
                if (isCurrentProvider) {
                  setIsYouTubeExpanded((expanded) => {
                    const nextExpanded = !expanded;
                    if (nextExpanded) void requestNavigation("youtube");
                    return nextExpanded;
                  });
                } else {
                  void requestNavigation("youtube");
                  setIsYouTubeExpanded(true);
                }
              }}
            >
              YouTube
            </SidebarItem>
            <div className="lv-nav-children" hidden={!isYouTubeExpanded}>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "youtube"}
                icon={<Download aria-hidden="true" />}
                aria-label="Download YouTube video"
                onClick={() => void requestNavigation("youtube")}
              >
                Download video
              </SidebarItem>
              <SidebarItem
                className="lv-nav-child"
                active={activeView === "youtube-history"}
                icon={<History aria-hidden="true" />}
                aria-label="YouTube downloaded history"
                onClick={() => void requestNavigation("youtube-history")}
              >
                Downloaded history
              </SidebarItem>
            </div>
          </div>
        </nav>

        <div className="lv-sidebar-footer">
          <SidebarItem className="lv-sidebar-settings" icon={<Settings aria-hidden="true" />} aria-label="Open settings" onClick={() => setIsSettingsOpen(true)}>Settings</SidebarItem>
          <div className="flex items-center gap-2">
            <Tooltip label={`Switch to ${theme === "dark" ? "day" : "night"} mode`}>
              <IconButton
                aria-label={`Switch to ${theme === "dark" ? "day" : "night"} mode`}
                aria-pressed={theme === "light"}
                onClick={() => setTheme((current) => current === "dark" ? "light" : "dark")}
              >
                {theme === "dark"
                  ? <SunMedium aria-hidden="true" className="h-4 w-4" />
                  : <Moon aria-hidden="true" className="h-4 w-4" />}
              </IconButton>
            </Tooltip>
            <Popover
              label="LinkedVault help"
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
              <div className="text-xs font-semibold text-muted-strong">LinkedVault</div>
              <p className="mt-2 text-xs leading-5 text-muted">
                LinkedIn and Coursera course downloads use a saved local session cookie. YouTube downloads public videos and playlists with transcripts; the signed helper pack is downloaded once after install when you open YouTube.
              </p>
            </Popover>
          </div>
        </div>
        <button
          type="button"
          className="lv-sidebar-rail"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          aria-valuemin={SIDEBAR_MIN_WIDTH}
          aria-valuemax={SIDEBAR_MAX_WIDTH}
          aria-valuenow={sidebarWidth}
          tabIndex={isSidebarCollapsed ? -1 : 0}
          onPointerDown={startSidebarResize}
          onKeyDown={(event) => {
            if (isSidebarCollapsed) return;
            const step = event.shiftKey ? 20 : 5;
            const currentWidth = liveSidebarWidth.current;
            let nextWidth = currentWidth;
            switch (event.key) {
              case "ArrowLeft":
                nextWidth = clampSidebarWidth(currentWidth - step);
                break;
              case "ArrowRight":
                nextWidth = clampSidebarWidth(currentWidth + step);
                break;
              case "Home":
                nextWidth = SIDEBAR_MIN_WIDTH;
                break;
              case "End":
                nextWidth = SIDEBAR_MAX_WIDTH;
                break;
              default:
                return;
            }
            event.preventDefault();
            // Update live ref immediately for single source of truth
            liveSidebarWidth.current = nextWidth;
            shellRef.current?.style.setProperty("--sidebar-width", `${nextWidth}px`);
            // Sync to React state for persistence
            setSidebarWidth(nextWidth);
          }}
        />
      </aside>
      <main className="lv-main" data-clipping-search={activeView === "newspaper-clippings" ? "true" : "false"}>
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
        {activeView === "newspaper-clippings" ? (
          <div
            className="lv-global-search"
            data-detail={isClippingDetailOpen ? "true" : "false"}
            role={isClippingDetailOpen ? undefined : "search"}
          >
            {isClippingDetailOpen ? (
              <>
                <button className="lv-global-search__back" onClick={() => void clippingNavigation.openGallery()} type="button">
                  <ArrowLeft aria-hidden="true" /> Back
                </button>
                <div className="lv-global-search__title-slot" id="clipping-detail-title-slot" />
              </>
            ) : (
              <>
                <Search aria-hidden="true" />
                <input
                  aria-label="Search saved newspaper clippings"
                  onChange={(event) => void updateGlobalSearch(event.target.value)}
                  placeholder="Search titles, notes, editions, dates, or pages"
                  value={globalSearchQuery}
                />
                {globalSearchQuery ? (
                  <button aria-label="Clear clipping search" onClick={() => void updateGlobalSearch("")} type="button">
                    <X aria-hidden="true" />
                  </button>
                ) : null}
                <div className="lv-global-search__aside" aria-label="Clipping view controls">
                  <div className="lv-global-search__context">
                    <strong>Clippings</strong>
                    <span>
                      {clippingGallerySummary?.loading
                        ? "Loading"
                        : `${clippingGallerySummary?.total ?? 0} clipping${(clippingGallerySummary?.total ?? 0) === 1 ? "" : "s"}`}
                    </span>
                  </div>
                  <div className="lv-global-search__view-toggle" role="group" aria-label="Clipping layout">
                    <IconButton
                      type="button"
                      size="icon-sm"
                      aria-label="Gallery view"
                      aria-pressed={clippingViewMode === "gallery"}
                      data-active={clippingViewMode === "gallery" ? "true" : undefined}
                      className="clipping-view-toggle"
                      onClick={() => writeClippingViewMode("gallery")}
                    >
                      <LayoutGrid aria-hidden="true" className="h-3.5 w-3.5" />
                    </IconButton>
                    <IconButton
                      type="button"
                      size="icon-sm"
                      aria-label="List view"
                      aria-pressed={clippingViewMode === "list"}
                      data-active={clippingViewMode === "list" ? "true" : undefined}
                      className="clipping-view-toggle"
                      onClick={() => writeClippingViewMode("list")}
                    >
                      <List aria-hidden="true" className="h-3.5 w-3.5" />
                    </IconButton>
                  </div>
                </div>
              </>
            )}
          </div>
        ) : null}
        <div className="lv-content" data-active-view={activeView}>
          {activeView === "newspaper-clippings" && activeSearchQuery && !isClippingDetailOpen && !clippingNavigation.pendingClippingId ? (
            <NewspaperClippingSearch query={activeSearchQuery} onOpen={(id) => void clippingNavigation.openClipping(id)} />
          ) : activeView === "coursera" ? (
            <CourseraView />
          ) : activeView === "coursera-history" ? (
            <CourseraView mode="history" />
          ) : activeView === "youtube" ? (
            <YouTubeView />
          ) : activeView === "youtube-history" ? (
            <YouTubeView mode="history" />
          ) : activeView === "newspaper-download" ? (
            <NewspaperView
              onRequestQueueProcess={(options) => ensureNewspaperQueueProcessing(options ?? null, true)}
            />
          ) : activeView === "newspaper-library" ? (
            <NewspaperView
              mode="library"
              onRequestQueueProcess={(options) => ensureNewspaperQueueProcessing(options ?? null, true)}
              onOpenClipping={(id) => void clippingNavigation.openClipping(id, true)}
              onReturnClipping={(id) => void clippingNavigation.openClipping(id, false, true)}
              onReaderTargetConsumed={clippingNavigation.consumeReaderTarget}
              readerTarget={clippingNavigation.pendingReaderTarget}
            />
          ) : activeView === "newspaper-clippings" ? (
            <NewspaperClippings
              key={clippingNavigation.clippingsViewKey}
              onGallerySummaryChange={setClippingGallerySummary}
              initialGalleryScrollTop={clippingGalleryScrollTopRef.current}
              onDetailStateChange={setIsClippingDetailOpen}
              onGalleryScrollTopChange={recordClippingGalleryScroll}
              onOpenLibrary={() => void requestNavigation("newspaper-library")}
              onOpenSource={(detail) => void clippingNavigation.openSource(detail)}
              onPendingConsumed={clippingNavigation.consumePendingClipping}
              pendingFocusEditor={clippingNavigation.pendingFocusEditor}
              pendingFocusSource={clippingNavigation.pendingFocusSource}
              pendingClippingId={clippingNavigation.pendingClippingId}
              registerFlush={registerClippingFlush}
            />
          ) : activeView === "linkedin-history" ? (
            <HistoryPage
              entries={downloadHistory}
              historyFilePath={downloadHistoryFilePath}
              onOpenFolderByJobId={openCompletedFolderByJobId}
            />
          ) : (
          <>
          <div className="lv-workspace linkedin-downloads-workspace">
            <div className="linkedin-search-stage">
              <div className="linkedin-search-field">
                <Textarea
                  ref={courseUrlsInputRef}
                  value={courseUrls}
                  onChange={(event) => {
                    setCourseUrls(event.target.value);
                    setParsedCourses([]);
                  }}
                  onBlur={validateUrls}
                  placeholder="Paste LinkedIn Learning course URLs"
                  spellCheck={false}
                  rows={1}
                  className="linkedin-search-input"
                  aria-label="Course URLs"
                />
              </div>

              <div className="linkedin-control-cluster">
                <label className="linkedin-cluster-field linkedin-option-quality">
                  <span>Quality</span>
                  <Select value={resolution} onChange={(event) => setResolution(event.target.value)} aria-label="Video resolution">
                    <option value="1080">1080 (Best)</option>
                    <option value="720">720 (High)</option>
                    <option value="540">540 (Medium)</option>
                    <option value="360">360 (Low)</option>
                  </Select>
                </label>
                <label className="linkedin-cluster-field linkedin-option-delay">
                  <span>Delay</span>
                  <div className="linkedin-delay-field">
                    <Input
                      value={delaySeconds}
                      type="number"
                      min={0}
                      max={DOWNLOAD_DELAY_MAX_SECONDS}
                      step={1}
                      onChange={(event) => updateDelaySeconds(event.target.value)}
                      aria-label="Delay seconds"
                      className="linkedin-delay-input"
                    />
                    <span className="linkedin-delay-unit" aria-hidden="true">sec</span>
                  </div>
                </label>
                <label className="linkedin-cluster-field linkedin-cluster-folder">
                  <span>Save to</span>
                  <button
                    type="button"
                    className="linkedin-folder-field"
                    onClick={() => void browseDownloadFolder()}
                    aria-label="LinkedIn folder"
                    title={folder || "Choose a folder"}
                  >
                    <Folder aria-hidden="true" />
                    <span className="linkedin-folder-path">{folder || "Choose a folder"}</span>
                  </button>
                </label>
                <label className="linkedin-cluster-field linkedin-cluster-session">
                  <span>Session</span>
                  <div className="linkedin-session-field">
                    <Input
                      value={token.length > 0 ? token : (hasSavedToken ? SAVED_TOKEN_MASK : "")}
                      onChange={(event) => {
                        const next = event.target.value;
                        if (token.length === 0 && hasSavedToken) {
                          if (next === SAVED_TOKEN_MASK) return;
                          setToken(next === SAVED_TOKEN_MASK ? "" : next);
                          return;
                        }
                        setToken(next);
                      }}
                      onFocus={(event) => {
                        if (token.length === 0 && hasSavedToken) {
                          event.currentTarget.select();
                        }
                      }}
                      placeholder="Paste li_at cookie"
                      type={token.length > 0 || !hasSavedToken ? "password" : "text"}
                      autoComplete="off"
                      spellCheck={false}
                      aria-label="LinkedIn li_at token"
                      title={hasSavedToken && !token ? "Saved LinkedIn session is available" : undefined}
                      className="linkedin-session-input"
                      data-has-saved={hasSavedToken && token.length === 0 ? "true" : undefined}
                    />
                    <div className="linkedin-session-actions">
                      <IconButton type="button" size="icon-sm" className="linkedin-session-action" onClick={() => setIsTokenGuideOpen(true)} aria-label="Open token guide" title="How to find your li_at cookie">
                        <CircleHelp aria-hidden="true" className="h-3.5 w-3.5" />
                      </IconButton>
                      <IconButton type="button" size="icon-sm" className="linkedin-session-action" onClick={clearToken} aria-label="Clear LinkedIn token" title="Clear saved session" disabled={!hasSavedToken && !token}>
                        <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                      </IconButton>
                    </div>
                  </div>
                </label>
              </div>

              <div className="linkedin-artifact-row">
                <div className="linkedin-artifact-toggles download-toggles" aria-label="Download artifacts">
                  <Checkbox checked={downloadVideos} onChange={(event) => setDownloadVideos(event.target.checked)} label="Videos" />
                  <Checkbox checked={downloadExercises} onChange={(event) => setDownloadExercises(event.target.checked)} label="Exercises" />
                  <Checkbox checked={downloadSubtitles} onChange={(event) => setDownloadSubtitles(event.target.checked)} label="Subtitles" />
                  <Checkbox checked={downloadQuizzes} onChange={(event) => setDownloadQuizzes(event.target.checked)} label="Quizzes" />
                  <label className="linkedin-video-wait" title="Random wait between LinkedIn video downloads. Changing this applies to the next wait; an in-progress wait finishes first.">
                    <span>Video wait</span>
                    <div className="linkedin-video-wait-fields">
                      <Input
                        type="number"
                        min={0}
                        max={VIDEO_WAIT_MAX_SECONDS}
                        step={1}
                        value={videoWaitMinSeconds}
                        onChange={(event) => updateVideoWaitBounds(event.target.value, videoWaitMaxSeconds)}
                        aria-label="Minimum seconds between video downloads"
                        className="linkedin-video-wait-input"
                      />
                      <span aria-hidden="true">–</span>
                      <Input
                        type="number"
                        min={0}
                        max={VIDEO_WAIT_MAX_SECONDS}
                        step={1}
                        value={videoWaitMaxSeconds}
                        onChange={(event) => updateVideoWaitBounds(videoWaitMinSeconds, event.target.value)}
                        aria-label="Maximum seconds between video downloads"
                        className="linkedin-video-wait-input"
                      />
                      <span className="linkedin-video-wait-unit" aria-hidden="true">sec</span>
                    </div>
                  </label>
                </div>
                <div className="linkedin-primary-actions">
                  <Button type="button" variant="primary" className="linkedin-action-button" onClick={() => void startDownload()} disabled={!canStart || isValidatingToken || isQueueingDownload}>
                    {isProcessingDownload ? <Plus aria-hidden="true" className="h-3.5 w-3.5" /> : <Play aria-hidden="true" className="h-3.5 w-3.5" />}
                    {isValidatingToken
                      ? "Validating"
                      : isQueueingDownload
                        ? isProcessingDownload ? "Adding" : "Queueing"
                        : isProcessingDownload ? "Add to queue" : "Download"}
                  </Button>
                  <Button type="button" variant="outline" className="linkedin-action-button" onClick={() => void openScheduleDialog()} disabled={!canStart || isValidatingToken || isQueueingDownload}>
                    <CalendarClock aria-hidden="true" className="h-3.5 w-3.5" />
                    Schedule
                  </Button>
                </div>
              </div>
            </div>

            <Panel className="table-panel queue-panel linkedin-queue-panel">
              <div className="table-panel-header">
                <h3 id="download-queue-heading">Queue</h3>
                <div className="table-panel-header-status linkedin-queue-header-actions">
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
                  <div className="linkedin-queue-controls" aria-label="LinkedIn queue controls">
                    <Button
                      type="button"
                      size="xs"
                      variant="outline"
                      onClick={() => void toggleAllDownloadsPause()}
                      disabled={pausableQueueJobs.length === 0 || pauseUpdatingTaskId !== null || isPausingAll}
                    >
                      {allPausableJobsPaused
                        ? <Play aria-hidden="true" className="h-3.5 w-3.5" />
                        : <Pause aria-hidden="true" className="h-3.5 w-3.5" />}
                      {isPausingAll ? "Updating" : allPausableJobsPaused ? "Resume all" : "Pause all"}
                    </Button>
                    <Button type="button" size="xs" variant="outline" onClick={cancelDownload} disabled={!activeDownloadJob || isCancellingDownload}>
                      <X aria-hidden="true" className="h-3.5 w-3.5" />
                      {isCancellingDownload ? "Cancelling" : "Cancel all"}
                    </Button>
                  </div>
                </div>
              </div>
              <div className="queue-section-tabs" role="group" aria-label="Download queue sections">
                <QueueSectionTab
                  section="queue"
                  label="Queue"
                  value={queueSectionCount}
                  tone="queue"
                  selected={queueSection === "queue"}
                  onClick={() => setQueueSection("queue")}
                />
                <QueueSectionTab
                  section="active"
                  label="Active"
                  value={activitySummary.active}
                  tone="primary"
                  selected={queueSection === "active"}
                  onClick={() => setQueueSection("active")}
                />
                <QueueSectionTab
                  section="completed"
                  label="Completed"
                  value={activitySummary.completed}
                  tone="success"
                  selected={queueSection === "completed"}
                  onClick={() => setQueueSection("completed")}
                />
                <QueueSectionTab
                  section="failed"
                  label="Failed"
                  value={activitySummary.failed}
                  tone="danger"
                  selected={queueSection === "failed"}
                  onClick={() => setQueueSection("failed")}
                />
              </div>
              <div
                className={`queue-section-panel queue-section-panel-${queueSection}`}
                aria-label={queueSection === "queue" ? "Download queue" : activityFilterLabel(queueSection)}
              >
                {queueSection === "queue" && queueNeedsSessionRefresh && (hasReadyQueuedJobs(queuedJobs) || failedJobs.length > 0) ? (
                  <div className="queue-session-warning" role="alert">
                    <div>
                      <strong>LinkedIn session needs refreshing.</strong>
                      <span>Paste a fresh li_at cookie, then resume.</span>
                    </div>
                    <Button
                      type="button"
                      size="xs"
                      variant="outline"
                      onClick={() => void resumeQueuedDownloads()}
                      disabled={isProcessingDownload || isValidatingToken}
                    >
                      <RotateCcw aria-hidden="true" className="h-3 w-3" />
                      {isValidatingToken ? "Validating" : "Resume queue"}
                    </Button>
                  </div>
                ) : null}
                <DownloadQueueTable
                  jobs={queueSection === "queue" ? displayedQueueJobs : queueSection === "active" ? activeJobs : queueSection === "completed" ? completedJobs : failedJobs}
                  parsedCourses={queueSection === "queue" ? parsedCourses : []}
                  hasPersistedJobs={queuedJobs.length > 0}
                  emptyTitle={queueSection === "queue" ? "No active downloads" : `No ${queueSection} downloads`}
                  emptyDescription=""
                  hideColumnHeader
                  onRetry={retryDownloadJob}
                  onCopyUrl={copyQueuedCourseUrl}
                  onRemove={removeQueueItem}
                  onOpenFolder={queueSection === "completed" ? openCompletedFolder : undefined}
                  onDownloadNow={downloadScheduledNow}
                  onPause={toggleDownloadPause}
                  pauseUpdatingTaskId={pauseUpdatingTaskId}
                  bulkPauseUpdating={isPausingAll}
                  showActiveDetails={queueSection === "queue" || queueSection === "active"}
                  recentEvents={allRecentEvents}
                />
              </div>
            </Panel>
          </div>
          </>
          )}
        </div>
      </main>
    </div>
    <Dialog
      open={isScheduleOpen}
      onOpenChange={(open) => {
        setIsScheduleOpen(open);
        if (!open) setScheduleStep("configure");
      }}
      title={scheduleStep === "configure" ? "Schedule course downloads" : "Confirm automatic schedule"}
      description={scheduleStep === "configure"
        ? `Choose a start window for ${scheduleCourseCount} course${scheduleCourseCount === 1 ? "" : "s"}. LinkedVault calculates bounded randomized pacing.`
        : "Review the queue behavior before LinkedVault saves the schedule."}
      className="schedule-dialog"
    >
      {scheduleStep === "configure" ? (
        <div className="schedule-config">
          <div className="schedule-field-grid">
            <Field label="Start within (hours)" className="schedule-window-field">
              <Input
                type="number"
                min={0}
                max={168}
                step={1}
                value={scheduleWindowHours}
                aria-label="Start within hours"
                onChange={(event) => setScheduleWindowHours(Number(event.target.value))}
              />
            </Field>
            <Field label="Plus minutes" className="schedule-window-field">
              <Input
                type="number"
                min={0}
                max={59}
                step={1}
                value={scheduleWindowMinutes}
                aria-label="Start within additional minutes"
                onChange={(event) => setScheduleWindowMinutes(Number(event.target.value))}
              />
            </Field>
            <Field label="Automatic minimum wait">
              <div className="schedule-auto-control">
                <output
                  aria-label="Automatic minimum wait minutes"
                  className="schedule-auto-output"
                >{formatScheduleDuration(scheduleMinWaitMinutes)}</output>
                <span>Auto</span>
              </div>
            </Field>
            <Field label="Automatic maximum wait">
              <div className="schedule-auto-control">
                <output
                  aria-label="Automatic maximum wait minutes"
                  className="schedule-auto-output"
                >{formatScheduleDuration(scheduleMaxWaitMinutes)}</output>
                <span>Auto</span>
              </div>
            </Field>
          </div>
          <div className="schedule-pacing-preview" aria-live="polite">
            <span>Calculated pace</span>
            <strong>
              About {formatScheduleDuration(automaticScheduleWaitRange.targetWaitMinutes)} per course,
              randomized from {formatScheduleDuration(scheduleMinWaitMinutes)} to {formatScheduleDuration(scheduleMaxWaitMinutes)}.
            </strong>
          </div>
          <div className="schedule-note">
            <Clock3 aria-hidden="true" />
            <div>
              <strong>Persistent queue</strong>
              <span>Schedules survive app restarts and new immediate downloads. LinkedVault runs due work while open and resumes overdue items the next time it launches.</span>
            </div>
          </div>
          <div className="schedule-actions">
            <Button type="button" variant="ghost" onClick={() => setIsScheduleOpen(false)}>Cancel</Button>
            <Button type="button" variant="primary" onClick={() => void reviewDownloadSchedule()}>Review schedule</Button>
          </div>
        </div>
      ) : (
        <div className="schedule-confirmation">
          <div className="schedule-confirmation-grid">
            <div><span>Courses</span><strong>{scheduleCourseCount}</strong></div>
            <div><span>Start within</span><strong>{formatScheduleDuration(scheduleWindowTotalMinutes)}</strong></div>
            <div><span>Random wait</span><strong>{scheduleMinWaitMinutes}–{scheduleMaxWaitMinutes}m</strong></div>
          </div>
          <p>The first course receives a randomized delay, and every following course is assigned a start time inside the selected window. Each item can still be started manually from the queue.</p>
          <div className="schedule-actions">
            <Button type="button" variant="ghost" onClick={() => setScheduleStep("configure")}>Back</Button>
            <Button type="button" variant="primary" loading={isQueueingDownload} loadingLabel="Scheduling" onClick={() => void confirmDownloadSchedule()}>
              <CalendarClock aria-hidden="true" className="h-3.5 w-3.5" />
              Confirm schedule
            </Button>
          </div>
        </div>
      )}
    </Dialog>
    <Dialog
      open={isSettingsOpen}
      onOpenChange={setIsSettingsOpen}
      title="LinkedVault settings"
      description="Save downloader defaults and session behavior without storing plaintext LinkedIn tokens." className="settings-dialog"
    >
      <div className="settings-grid">
        <section className="settings-section">
          <div className="settings-section-title">LinkedIn download defaults</div>
          <Field label="LinkedIn download folder">
            <div className="field-action-grid">
              <Input value={folder} onChange={(event) => setFolder(event.target.value)} aria-label="LinkedIn download folder" />
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
            <Field label="Delay between courses (seconds)">
              <Input
                value={delaySeconds}
                type="number"
                min={0}
                max={DOWNLOAD_DELAY_MAX_SECONDS}
                step={1}
                onChange={(event) => updateDelaySeconds(event.target.value)}
                aria-label="Settings delay seconds"
              />
            </Field>
          </div>
          <Field label="Browser source">
            <Select value={browserSource} onChange={(event) => setBrowserSource(event.target.value)} aria-label="Settings browser source">
              {browserSources.map((source) => <option key={source} value={source}>{source}</option>)}
            </Select>
          </Field>
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
          <div className="settings-section-title">Newspaper</div>
          <div className="settings-newspaper-grid">
            <Field label="Default zoom level">
              <Select
                value={String(Math.round(newspaperDefaultZoom * 100))}
                onChange={(event) => {
                  const nextDefault = Number(event.target.value) / 100;
                  setNewspaperDefaultZoom(nextDefault);
                  if (newspaperClickZoom <= nextDefault) {
                    setNewspaperClickZoom(clampNewspaperReaderZoom(nextDefault + .2));
                  }
                }}
                aria-label="Default newspaper zoom"
              >
                {NEWSPAPER_READER_ZOOM_OPTIONS.map((value) => (
                  <option key={value} value={Math.round(value * 100)}>{Math.round(value * 100)}%</option>
                ))}
              </Select>
            </Field>
            <Field label="Left-click zoom level">
              <Select
                value={String(Math.round(newspaperClickZoom * 100))}
                onChange={(event) => setNewspaperClickZoom(Number(event.target.value) / 100)}
                aria-label="Newspaper left-click zoom"
              >
                {NEWSPAPER_READER_ZOOM_OPTIONS.map((value) => (
                  <option
                    key={value}
                    value={Math.round(value * 100)}
                    disabled={value <= newspaperDefaultZoom && value < NEWSPAPER_READER_ZOOM_MAX}
                  >
                    {Math.round(value * 100)}%
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Default page tone">
              <Select
                value={newspaperPageTone}
                onChange={(event) => setNewspaperPageTone(event.target.value as NewspaperPageTone)}
                aria-label="Default newspaper page tone"
              >
                {NEWSPAPER_PAGE_TONES.map((tone) => (
                  <option key={tone} value={tone}>{NEWSPAPER_PAGE_TONE_LABELS[tone]}</option>
                ))}
              </Select>
            </Field>
          </div>
          <div className="settings-button-row">
            <Button
              type="button"
              variant="outline"
              onClick={() => void registerNewspaperArchive()}
              loading={isRegisteringNewspaperArchive}
              loadingLabel="Recovering"
            >
              <FolderOpen aria-hidden="true" className="h-3.5 w-3.5" />
              Recover newspaper archive
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => void repairNewspaperLibrary()}
              loading={isRepairingNewspaperLibrary}
              loadingLabel="Repairing"
            >
              <RotateCcw aria-hidden="true" className="h-3.5 w-3.5" />
              Repair existing
            </Button>
          </div>
          <NewspaperSnapshotRootsSettings open={isSettingsOpen} />
          <div className="settings-section-subtitle">Optimization governor</div>
          <div className="settings-two-column">
            <Field label="Memory per worker (MB)">
              <Input
                type="number"
                min={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.workerMemoryBudgetMb.min}
                max={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.workerMemoryBudgetMb.max}
                step={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.workerMemoryBudgetMb.step}
                value={newspaperOptimizationPreferences.workerMemoryBudgetMb}
                onChange={(event) => {
                  const next = Number(event.target.value);
                  setNewspaperOptimizationPreferences((previous) => ({
                    ...previous,
                    workerMemoryBudgetMb: Number.isFinite(next) ? next : previous.workerMemoryBudgetMb
                  }));
                }}
                onBlur={() => writeNewspaperOptimizationPreferences(newspaperOptimizationPreferences)}
                aria-label="Newspaper optimization memory per worker"
              />
            </Field>
            <Field label="Memory reserve (MB)">
              <Input
                type="number"
                min={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.memoryReserveMb.min}
                max={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.memoryReserveMb.max}
                step={NEWSPAPER_OPTIMIZATION_MEMORY_BOUNDS.memoryReserveMb.step}
                value={newspaperOptimizationPreferences.memoryReserveMb}
                onChange={(event) => {
                  const next = Number(event.target.value);
                  setNewspaperOptimizationPreferences((previous) => ({
                    ...previous,
                    memoryReserveMb: Number.isFinite(next) ? next : previous.memoryReserveMb
                  }));
                }}
                onBlur={() => writeNewspaperOptimizationPreferences(newspaperOptimizationPreferences)}
                aria-label="Newspaper optimization memory reserve"
              />
            </Field>
          </div>
          <p className="settings-hint">
            Auto mode caps the optimization at 50% CPU and adjusts the worker
            pool every 3 seconds. 4K and other memory-hungry editions may
            need a larger per-worker budget; the reserve must stay large
            enough for the rest of the OS, the LinkedVault UI, and the active
            download.
          </p>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">Application</div>
          <div className="settings-row">
            <span>Theme</span>
            <span>{theme === "dark" ? "Night" : "Day"}</span>
          </div>
          <div className="settings-row">
            <span>Version</span>
            <span>v{pendingUpdate?.current_version ?? APP_VERSION}</span>
          </div>
          <div className="settings-row">
            <span>Update status</span>
            <span className={pendingUpdate ? "text-success" : "text-muted"}>
              {pendingUpdate ? `v${pendingUpdate.version} available` : "No pending update"}
            </span>
          </div>
          <div className="settings-button-row">
            <Button type="button" variant="outline" onClick={checkForUpdates} loading={isCheckingUpdate} loadingLabel="Checking">
              <RotateCcw aria-hidden="true" className="h-3.5 w-3.5" />
              Check for updates
            </Button>
            <Button type="button" variant="primary" onClick={() => void installUpdate()} disabled={!pendingUpdate} loading={isInstallingUpdate} loadingLabel="Installing">
              <Play aria-hidden="true" className="h-3.5 w-3.5" />
              Install update
            </Button>
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">Testing</div>
          <div className="settings-row">
            <span>Local-only download emulator</span>
            <span className={emulatorJobs.length > 0 ? "text-success" : "text-muted"}>
              {emulatorJobs.length > 0 ? `${emulatorJobs.length} mock rows` : "Idle"}
            </span>
          </div>
          <p className="settings-section-description">
            Seeds four fake LinkedIn queue rows so you can test the progress overlay sitting on top of the downloads below. Click a downloading course in Queue to open it.
          </p>
          <div className="settings-button-row">
            <Button type="button" variant="outline" onClick={startDownloadEmulator}>
              <Play aria-hidden="true" className="h-3.5 w-3.5" />
              {emulatorJobs.length > 0 ? "Restart emulator" : "Run download emulator"}
            </Button>
            {emulatorJobs.length > 0 ? (
              <Button type="button" variant="outline" onClick={() => stopDownloadEmulator()}>
                <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                Clear test
              </Button>
            ) : null}
            <Button type="button" variant="outline" onClick={() => void startYouTubeDownloadMock()}>
              <Play aria-hidden="true" className="h-3.5 w-3.5" />
              Mock YouTube download
            </Button>
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-title">Data management</div>
          <p className="settings-section-description">
            Clear a provider's in-app database without touching the files you have already downloaded to disk.
            The saved LinkedIn token is preserved.
          </p>
          <div className="settings-button-row">
            <Button
              type="button"
              variant="outline"
              onClick={() => requestResetProvider("linkedin")}
              disabled={resetInProgress === "linkedin" || pausingForReset === "linkedin"}
            >
              <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
              {pausingForReset === "linkedin"
                ? "Pausing LinkedIn"
                : resetInProgress === "linkedin"
                  ? "Clearing LinkedIn"
                  : "Reset LinkedIn database"}
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => requestResetProvider("coursera")}
              disabled={resetInProgress === "coursera" || pausingForReset === "coursera"}
            >
              <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
              {pausingForReset === "coursera"
                ? "Pausing Coursera"
                : resetInProgress === "coursera"
                  ? "Clearing Coursera"
                  : "Reset Coursera database"}
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => requestResetProvider("newspaper")}
              disabled={resetInProgress === "newspaper" || pausingForReset === "newspaper"}
            >
              <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
              {pausingForReset === "newspaper"
                ? "Pausing World Journal"
                : resetInProgress === "newspaper"
                  ? "Clearing World Journal"
                  : "Reset World Journal database"}
            </Button>
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
    <Dialog
      open={isTokenGuideOpen}
      onOpenChange={handleTokenGuideOpenChange}
      title="Find your LinkedIn li_at cookie"
      description="Use this only with a LinkedIn Learning account you are allowed to access. LinkedVault saves the cookie locally with Windows encryption."
      className="token-guide-dialog"
    >
      <div className="token-guide-content">
        <img src={liAtCookieGuide} alt="Chrome DevTools showing the Application tab, Cookies storage, and the li_at cookie row." />
        <ol className="token-guide-steps">
          <li>Open LinkedIn Learning in your browser and sign in.</li>
          <li>Press F12, then open the Application tab.</li>
          <li>Under Storage, open Cookies and choose https://www.linkedin.com.</li>
          <li>Find li_at, copy its full Value, and paste it into LinkedVault.</li>
        </ol>
        <div className="token-guide-actions">
          <Button type="button" variant="primary" onClick={() => handleTokenGuideOpenChange(false)}>
            Got it
          </Button>
        </div>
      </div>
    </Dialog>
    <Dialog
      open={pendingResetProvider !== null}
      onOpenChange={(open) => {
        if (!open) setPendingResetProvider(null);
      }}
      title={pendingResetProvider ? `Reset ${resetProviderLabel(pendingResetProvider)} database?` : "Reset database"}
      description="This is destructive. Read the details before you continue."
    >
      {pendingResetProvider ? (
        <div className="reset-confirm">
          {pendingResetProvider === "newspaper" ? (
            <p>
              Resetting World Journal removes downloaded-edition records, reading progress,
              schedules, and generated newspaper previews. <strong>Your saved clippings and
              clipping notes are preserved.</strong>
            </p>
          ) : (
            <p>
              Clearing the {resetProviderLabel(pendingResetProvider)} database removes the in-app records
              for that provider. Files you have already saved to your download folder are <strong>not</strong> deleted.
              Your saved LinkedIn <code>li_at</code> cookie is preserved.
            </p>
          )}
          <ul className="reset-confirm-list">
            {pendingResetProvider === "linkedin" ? (
              <>
                <li>LinkedIn download queue, history, and saved preferences</li>
                <li>LinkedIn course discovery cache</li>
                <li>The Markdown download history file is rewritten as empty</li>
              </>
            ) : null}
            {pendingResetProvider === "coursera" ? (
              <>
                <li>Coursera download queue, history, and per-course options</li>
                <li>Coursera provider preferences</li>
              </>
            ) : null}
            {pendingResetProvider === "newspaper" ? (
              <>
                <li>World Journal editions, batches, jobs, and pages</li>
                <li>Thumbnail cache, optimization ledger, reading progress</li>
                <li>Newspaper schedules and provider settings</li>
                <li>On-disk <code>newspaper-thumbnails/</code> directory</li>
                <li>Saved clipping images, notes, and Snapshot locations are preserved</li>
              </>
            ) : null}
          </ul>
          <p>
            {pendingResetProvider === "linkedin" && activeLinkedinJobCount() > 0
              ? `${activeLinkedinJobCount()} download${activeLinkedinJobCount() === 1 ? " is" : "s are"} still in flight. LinkedVault will pause them at the next safe boundary before wiping.`
              : "No active downloads detected for this provider."}
          </p>
          <div className="reset-confirm-actions">
            <Button type="button" variant="ghost" onClick={() => setPendingResetProvider(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="primary"
              onClick={() => void performProviderReset(pendingResetProvider)}
            >
              <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
              Reset {resetProviderLabel(pendingResetProvider)} database
            </Button>
          </div>
        </div>
      ) : null}
    </Dialog>
    </>
  );
}

function shouldShowInLiveQueue(status: string) {
  return status !== "completed" && status !== "cancelled";
}

function isDownloadEmulatorJob(job: QueuedDownloadJob) {
  return job.is_test_emulator === true;
}

function createDownloadEmulatorJob({
  id,
  courseSlug,
  title,
  status,
  timestamp,
  paused = false,
  artifactCounts,
  videoArtifacts
}: {
  id: string;
  courseSlug: string;
  title: string;
  status: string;
  timestamp: number;
  paused?: boolean;
  artifactCounts?: ArtifactProgressCounts;
  videoArtifacts?: VideoDownloadArtifact[];
}): QueuedDownloadJob {
  return {
    id,
    course_slug: courseSlug,
    source_url: `test://linkvault/download-emulator/${courseSlug}`,
    status,
    title,
    thumbnail_url: null,
    selected_quality: "720",
    output_dir: "",
    paused,
    scheduled_at: null,
    created_at: timestamp,
    updated_at: timestamp,
    artifact_counts: artifactCounts ?? emptyArtifactCounts(),
    video_artifacts: videoArtifacts ?? [],
    is_test_emulator: true
  };
}

function isScheduledJob(job: QueuedDownloadJob) {
  return job.status === "queued" && !job.paused && typeof job.scheduled_at === "number";
}

function hasReadyQueuedJobs(jobs: QueuedDownloadJob[]) {
  const now = Math.floor(Date.now() / 1000);
  return jobs.some((job) => job.status === "queued" && !job.paused && (!job.scheduled_at || job.scheduled_at <= now));
}

function formatScheduledDate(timestamp: number) {
  if (!timestamp) return "schedule pending";
  return new Date(timestamp * 1000).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit"
  });
}

function formatScheduledTime(timestamp: number) {
  if (!timestamp) return "--:--";
  return new Date(timestamp * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function formatScheduleDuration(minutes: number) {
  if (minutes < 60) return `${minutes} min`;
  const wholeHours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `${wholeHours} hr ${remainingMinutes} min` : `${wholeHours} hr`;
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
  return [...latestByCourse.values()].sort(
    (first, second) =>
      (second.updated_at ?? 0) - (first.updated_at ?? 0) ||
      (second.created_at ?? 0) - (first.created_at ?? 0) ||
      second.id.localeCompare(first.id)
  );
}

function jobsForActivityFilter(jobs: QueuedDownloadJob[], filter: ActivityFilter) {
  const matchingJobs = jobs.filter((job) => {
    if (filter === "failed") return job.status === "failed" || job.status === "cancelled";
    return job.status === filter;
  });
  return filter === "completed" ? completedCourseJobs(matchingJobs) : matchingJobs.sort(
    (first, second) =>
      (second.updated_at ?? 0) - (first.updated_at ?? 0) ||
      second.id.localeCompare(first.id)
  );
}

function activityFilterLabel(filter: ActivityFilter) {
  if (filter === "active") return "Active downloads";
  if (filter === "completed") return "Completed downloads";
  return "Failed downloads";
}

function mergeQueuedJobs(currentJobs: QueuedDownloadJob[], addedJobs: QueuedDownloadJob[]) {
  const jobsById = new Map(currentJobs.map((job) => [job.id, job]));
  for (const job of addedJobs) {
    jobsById.set(job.id, job);
  }
  return [...jobsById.values()].sort(
    (first, second) =>
      (second.updated_at ?? 0) - (first.updated_at ?? 0) ||
      (second.created_at ?? 0) - (first.created_at ?? 0) ||
      second.id.localeCompare(first.id)
  );
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

function activeVideoPacingState(
  events: PersistedJobEvent[],
  jobId: string,
  videoArtifacts: VideoDownloadArtifact[],
  nowSeconds: number
): VideoPacingState | null {
  const queuedVideoIds = new Set(
    videoArtifacts
      .filter((artifact) => artifact.status === "pending" || artifact.status === "active")
      .map((artifact) => artifact.id)
  );
  const pacingEvents = events
    .filter((event) => event.job_id === jobId && event.event_type === "video.pacing.wait")
    .sort((left, right) => right.created_at - left.created_at || right.id - left.id);

  for (const event of pacingEvents) {
    const pacing = parseVideoPacingState(event.payload_json);
    if (!pacing || pacing.wait_until <= nowSeconds) continue;
    if (queuedVideoIds.size > 0 && !queuedVideoIds.has(pacing.artifact_id)) continue;
    return pacing;
  }
  return null;
}

function parseVideoPacingState(payloadJson?: string | null): VideoPacingState | null {
  if (!payloadJson) return null;
  try {
    const parsed: unknown = JSON.parse(payloadJson);
    if (!isUnknownRecord(parsed)) return null;
    const artifactId = parsed.artifactId;
    const waitSeconds = parsed.waitSeconds;
    const waitStartedAt = parsed.waitStartedAt;
    const waitUntil = parsed.waitUntil;
    if (
      typeof artifactId !== "string" ||
      !artifactId.trim() ||
      typeof waitSeconds !== "number" ||
      !Number.isFinite(waitSeconds) ||
      typeof waitStartedAt !== "number" ||
      !Number.isFinite(waitStartedAt) ||
      typeof waitUntil !== "number" ||
      !Number.isFinite(waitUntil)
    ) {
      return null;
    }
    return {
      artifact_id: artifactId,
      wait_seconds: Math.max(0, Math.round(waitSeconds)),
      wait_started_at: Math.round(waitStartedAt),
      wait_until: Math.round(waitUntil)
    };
  } catch {
    return null;
  }
}

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function formatCountdown(totalSeconds: number) {
  const boundedSeconds = Math.max(0, Math.ceil(totalSeconds));
  const minutes = Math.floor(boundedSeconds / 60);
  const seconds = boundedSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function videoArtifactStatusLabel(status: string) {
  if (status === "active") return "Downloading";
  if (status === "completed") return "Complete";
  if (status === "failed") return "Failed";
  if (status === "cancelled") return "Cancelled";
  if (status === "skipped") return "Skipped";
  return "Queued";
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

function QueueSectionTab({
  section,
  label,
  value,
  tone,
  selected,
  onClick
}: {
  section: DownloadQueueSection;
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

function DownloadQueueTable({
  jobs,
  parsedCourses,
  hasPersistedJobs,
  emptyTitle = "No active downloads",
  emptyDescription,
  hideColumnHeader = false,
  onRetry,
  onCopyUrl,
  onRemove,
  onOpenFolder,
  onDownloadNow,
  onPause,
  pauseUpdatingTaskId,
  bulkPauseUpdating,
  showActiveDetails,
  recentEvents
}: {
  jobs: QueuedDownloadJob[];
  parsedCourses: ParsedCourse[];
  hasPersistedJobs: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  hideColumnHeader?: boolean;
  onRetry: (job: QueuedDownloadJob) => void | Promise<void>;
  onCopyUrl: (job: QueuedDownloadJob) => void | Promise<void>;
  onRemove: (job: QueuedDownloadJob) => void | Promise<void>;
  onOpenFolder?: (job: QueuedDownloadJob) => void | Promise<void>;
  onDownloadNow: (job: QueuedDownloadJob) => void | Promise<void>;
  onPause: (job: QueuedDownloadJob) => void | Promise<void>;
  pauseUpdatingTaskId: string | null;
  bulkPauseUpdating: boolean;
  showActiveDetails: boolean;
  recentEvents: PersistedJobEvent[];
}) {
  const [expandedJobId, setExpandedJobId] = useState<string | null>(null);

  useEffect(() => {
    if (!showActiveDetails) {
      setExpandedJobId(null);
      return;
    }
    if (expandedJobId && !jobs.some((job) => job.id === expandedJobId)) {
      setExpandedJobId(null);
    }
  }, [expandedJobId, jobs, showActiveDetails]);

  useEffect(() => {
    if (!expandedJobId) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setExpandedJobId(null);
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [expandedJobId]);

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
          <QueueJobRow
            key={job.id}
            job={job}
            onRetry={onRetry}
            onCopyUrl={onCopyUrl}
            onRemove={onRemove}
            onOpenFolder={onOpenFolder}
            onDownloadNow={onDownloadNow}
            onPause={onPause}
            pauseUpdatingTaskId={pauseUpdatingTaskId}
            bulkPauseUpdating={bulkPauseUpdating}
            showActiveDetails={showActiveDetails}
            isExpanded={expandedJobId === job.id}
            onToggleExpanded={() => setExpandedJobId((current) => current === job.id ? null : job.id)}
            recentEvents={recentEvents}
          />
        ))
      ) : parsedCourses.length > 0 ? (
        parsedCourses.map((course, index) => <ValidatedQueueRow key={`${course.slug}-${index}`} course={course} />)
      ) : (
        <EmptyRow
          title={emptyTitle}
          description={emptyDescription ?? (hasPersistedJobs ? "Finished courses are in Completed. Failed jobs stay here until handled." : "Active jobs and items needing attention appear here after Download.")}
        />
      )}
    </DataTable>
  );
}

function QueueJobRow({
  job,
  onRetry,
  onCopyUrl,
  onRemove,
  onOpenFolder,
  onDownloadNow,
  onPause,
  pauseUpdatingTaskId,
  bulkPauseUpdating,
  showActiveDetails,
  isExpanded,
  onToggleExpanded,
  recentEvents
}: {
  job: QueuedDownloadJob;
  onRetry: (job: QueuedDownloadJob) => void | Promise<void>;
  onCopyUrl: (job: QueuedDownloadJob) => void | Promise<void>;
  onRemove: (job: QueuedDownloadJob) => void | Promise<void>;
  onOpenFolder?: (job: QueuedDownloadJob) => void | Promise<void>;
  onDownloadNow: (job: QueuedDownloadJob) => void | Promise<void>;
  onPause: (job: QueuedDownloadJob) => void | Promise<void>;
  pauseUpdatingTaskId: string | null;
  bulkPauseUpdating: boolean;
  showActiveDetails: boolean;
  isExpanded: boolean;
  onToggleExpanded: () => void;
  recentEvents: PersistedJobEvent[];
}) {
  const counts = artifactCounts(job);
  const progress = courseOverallProgress(job, counts);
  const title = courseDisplayName(job);
  const queueLabel = queueCourseLabel(job, counts);
  const videoArtifacts = job.video_artifacts ?? [];
  const canRemove = true;
  const scheduled = isScheduledJob(job);
  const [clockMs, setClockMs] = useState(() => Date.now());
  const detailsId = `queue-details-${job.id}`;
  const pacing = showActiveDetails
    ? activeVideoPacingState(recentEvents, job.id, videoArtifacts, Math.floor(clockMs / 1000))
    : null;
  const removeLabel = job.status === "active"
    ? "Cancel and remove"
    : job.status === "failed" || job.status === "cancelled"
      ? "Clear failed attempt"
      : "Remove from queue";

  useEffect(() => {
    if (!showActiveDetails || !isExpanded) return;
    const intervalId = window.setInterval(() => setClockMs(Date.now()), 1000);
    return () => window.clearInterval(intervalId);
  }, [isExpanded, showActiveDetails]);

  return (
    <div className={`queue-job-stack${isExpanded ? " is-open" : ""}`}>
      <DataTableRow
        className={`queue-table-row${showActiveDetails ? " is-expandable" : ""}${job.status === "active" || job.paused ? " has-persistent-actions" : ""}`}
        title="Right-click to copy this course URL"
        onClick={(event) => {
          if (!showActiveDetails) return;
          if (event.target instanceof Element && event.target.closest("button, a")) return;
          onToggleExpanded();
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          void onCopyUrl(job);
        }}
      >
        <QueueStatusBadge job={job} title={title} onRetry={onRetry} />
        <div className="table-course-cell">
          {showActiveDetails ? (
            <Tooltip label={isExpanded ? "Hide video queue details" : "Show video queue details"}>
              <button
                type="button"
                className={`queue-row-disclosure${isExpanded ? " is-expanded" : ""}`}
                aria-label={`${isExpanded ? "Hide" : "Show"} video queue details for ${title}`}
                aria-expanded={isExpanded}
                aria-controls={detailsId}
                onClick={() => onToggleExpanded()}
              >
                <ChevronDown aria-hidden="true" className="h-3.5 w-3.5" />
              </button>
            </Tooltip>
          ) : null}
          {job.thumbnail_url ? <MiniCourseArt title={title} thumbnailUrl={job.thumbnail_url} /> : (
            <span
              className={`course-status-mark ${job.status === "active" && !job.paused ? "queue-live-dot" : activityDotClass(job.status === "queued" ? "muted" : eventTone(job.status))}`}
            />
          )}
          <div className="min-w-0">
            <div className="truncate font-medium" title={title}>{queueLabel}</div>
            <div className="truncate text-soft" title={scheduled ? formatScheduledDate(job.scheduled_at ?? 0) : job.source_url}>
              {scheduled ? `Runs ${formatScheduledDate(job.scheduled_at ?? 0)}` : filesSummaryText(counts, job.status)}
            </div>
          </div>
        </div>
        <div className={`table-progress-cell${job.status === "active" && !job.paused ? " queue-live-progress" : ""}`}>
          {scheduled ? <span className="scheduled-time-compact">{formatScheduledTime(job.scheduled_at ?? 0)}</span> : <><Progress value={progress} /><span>{progress}%</span></>}
        </div>
        <div className="queue-row-actions">
          {job.status === "completed" && onOpenFolder ? (
            <Tooltip label="Open output folder">
              <IconButton
                type="button"
                aria-label={`Open output folder for ${title}`}
                onClick={() => void onOpenFolder(job)}
                className="queue-open-folder-button"
                disabled={!job.output_dir}
              >
                <FolderOpen aria-hidden="true" className="h-3.5 w-3.5" />
              </IconButton>
            </Tooltip>
          ) : null}
          {scheduled && !job.paused ? (
            <Tooltip label="Download now">
              <IconButton
                type="button"
                aria-label={`Download ${title} now`}
                onClick={() => onDownloadNow(job)}
                className="scheduled-download-now"
              >
                <Play aria-hidden="true" className="h-3.5 w-3.5" />
              </IconButton>
            </Tooltip>
          ) : null}
          {(job.status === "active" || job.status === "queued") && !job.is_test_emulator ? (
            <Tooltip label={job.paused ? "Resume download" : "Pause download"}>
              <IconButton
                type="button"
                aria-label={`${job.paused ? "Resume" : "Pause"} ${title}`}
                onClick={() => onPause(job)}
                className="queue-pause-button"
                loading={pauseUpdatingTaskId === job.id}
                disabled={pauseUpdatingTaskId !== null || bulkPauseUpdating}
              >
                {job.paused
                  ? <Play aria-hidden="true" className="h-3.5 w-3.5" />
                  : <Pause aria-hidden="true" className="h-3.5 w-3.5" />}
              </IconButton>
            </Tooltip>
          ) : null}
          {canRemove ? (
            <Tooltip label={removeLabel}>
              <IconButton
                type="button"
                aria-label={`${removeLabel}: ${title}`}
                onClick={() => onRemove(job)}
                className="queue-remove-button"
              >
                <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
              </IconButton>
            </Tooltip>
          ) : null}
        </div>
      </DataTableRow>
      {showActiveDetails && isExpanded ? (
        <ActiveQueueDetails
          id={detailsId}
          job={job}
          counts={counts}
          progress={progress}
          videoArtifacts={videoArtifacts}
          pacing={pacing}
          nowSeconds={Math.floor(clockMs / 1000)}
        />
      ) : null}
    </div>
  );
}

function ActiveQueueDetails({
  id,
  job,
  counts,
  progress,
  videoArtifacts,
  pacing,
  nowSeconds
}: {
  id: string;
  job: QueuedDownloadJob;
  counts: ArtifactProgressCounts;
  progress: number;
  videoArtifacts: VideoDownloadArtifact[];
  pacing: VideoPacingState | null;
  nowSeconds: number;
}) {
  const currentVideo = videoArtifacts.find((artifact) => artifact.status === "active") ?? null;
  const pacingVideo = pacing
    ? videoArtifacts.find((artifact) => artifact.id === pacing.artifact_id) ?? null
    : null;
  const completedVideos = videoArtifacts.filter((artifact) => artifact.status === "completed").length;
  const nextQueuedVideo = videoArtifacts.find((artifact) => artifact.status === "pending") ?? null;
  const accountedArtifacts = counts.completed + counts.failed + counts.cancelled;

  return (
    <div id={id} className="queue-detail-row queue-detail-overlay" role="region" aria-label={`Video queue details for ${courseDisplayName(job)}`}>
      <div className="queue-detail-inner">
        <div className="queue-detail-header">
          <div className="min-w-0">
            <span className="queue-detail-kicker">Course videos</span>
            <strong>{videoArtifacts.length > 0 ? `${completedVideos} of ${videoArtifacts.length} videos complete` : "Preparing video queue"}</strong>
          </div>
          <span className="queue-detail-overall">{progress}% overall</span>
        </div>

        <div className="queue-detail-progress queue-live-progress">
          <Progress value={progress} />
          <span>{counts.total > 0 ? `${accountedArtifacts} of ${counts.total} files accounted for` : "Waiting for the artifact plan"}</span>
        </div>

        {pacing ? (
          <div className="queue-detail-pacing" aria-live="polite">
            <Clock3 aria-hidden="true" className="h-3.5 w-3.5" />
            <div className="min-w-0">
              <strong>Random cooldown</strong>
              <span>{pacingVideo ? `before ${pacingVideo.display_name}` : "before the next video request"}</span>
            </div>
            <strong className="queue-detail-pacing-countdown">{formatCountdown(Math.max(0, pacing.wait_until - nowSeconds))}</strong>
            <span className="queue-detail-pacing-window">{pacing.wait_seconds}s window</span>
          </div>
        ) : currentVideo ? (
          <div className="queue-detail-current">
            <span className="queue-detail-current-dot is-active" aria-hidden="true" />
            <div className="min-w-0">
              <span className="queue-detail-kicker">Now downloading</span>
              <strong className="truncate" title={currentVideo.display_name}>{currentVideo.display_name}</strong>
            </div>
            <span className="queue-detail-current-status">In progress</span>
          </div>
        ) : nextQueuedVideo ? (
          <div className="queue-detail-current">
            <span className="queue-detail-current-dot" aria-hidden="true" />
            <div className="min-w-0">
              <span className="queue-detail-kicker">Next in queue</span>
              <strong className="truncate" title={nextQueuedVideo.display_name}>{nextQueuedVideo.display_name}</strong>
            </div>
            <span className="queue-detail-current-status">Waiting</span>
          </div>
        ) : (
          <div className="queue-detail-current is-muted">
            <span className="queue-detail-current-dot" aria-hidden="true" />
            <div className="min-w-0">
              <span className="queue-detail-kicker">Queue status</span>
              <strong>{videoArtifacts.length > 0 ? "Processing other course files" : "Fetching course details"}</strong>
            </div>
          </div>
        )}

        {videoArtifacts.length > 0 ? (
          <div className="queue-video-list" role="list" aria-label="Queued course videos">
            {videoArtifacts.map((artifact) => (
              <div key={artifact.id} className="queue-video-item" role="listitem">
                <span className={`queue-video-status-dot status-${artifact.status}`} aria-hidden="true" />
                <span className="queue-video-name truncate" title={artifact.display_name}>{artifact.display_name}</span>
                <span className={`queue-video-status status-${artifact.status}`}>{videoArtifactStatusLabel(artifact.status)}</span>
              </div>
            ))}
          </div>
        ) : null}
      </div>
    </div>
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
      <span aria-hidden="true" />
    </DataTableRow>
  );
}

function QueueStatusBadge({ job, title, onRetry }: { job: QueuedDownloadJob; title: string; onRetry: (job: QueuedDownloadJob) => void | Promise<void> }) {
  if (job.paused) {
    return (
      <StatusBadge className="paused-status-pill" dotClassName="bg-warning">
        <Pause aria-hidden="true" className="h-3 w-3" />
        <span>Paused</span>
      </StatusBadge>
    );
  }
  if (isScheduledJob(job)) {
    return (
      <StatusBadge className="scheduled-status-pill" dotClassName="bg-primary">
        <Clock3 aria-hidden="true" className="h-3 w-3" />
        <span>Scheduled</span>
      </StatusBadge>
    );
  }
  if (job.status === "active") {
    return (
      <StatusBadge className="queue-live-status-pill" dotClassName="queue-live-dot">
        <span>Downloading</span>
      </StatusBadge>
    );
  }
  return (
    <StatusBadge className={jobStatusBadgeClass(job.status)} dotClassName={activityDotClass(eventTone(job.status))}>
      <span>{jobStatusLabel(job.status, job.paused)}</span>
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

function HistoryPage({
  entries,
  historyFilePath,
  onOpenFolderByJobId
}: {
  entries: DownloadHistoryEntry[];
  historyFilePath: string;
  onOpenFolderByJobId: (jobId: string, fallbackPath?: string) => void | Promise<void>;
}) {
  return (
    <div className="lv-workspace download-history-workspace">
      <div className="download-history-header">
        <p className="download-history-count">
          {entries.length} completed course{entries.length === 1 ? "" : "s"}
        </p>
        {historyFilePath ? (
          <p className="download-history-meta" title={historyFilePath}>
            {historyFilePath}
          </p>
        ) : null}
      </div>
      {entries.length === 0 ? (
        <div className="download-history-empty" role="status">
          <span>No downloaded courses</span>
          <span>Completed course downloads will appear here and in download-history.md.</span>
        </div>
      ) : (
        <ol className="download-history-list" aria-label="LinkedIn download history">
          {entries.map((entry) => {
            const when = formatEventTime(entry.completed_at);
            return (
              <li key={entry.job_id} className="download-history-row">
                <div className="download-history-copy">
                  <strong title={entry.course_title}>{entry.course_title}</strong>
                  <span title={entry.source_url}>
                    {[when, entry.source_url].filter(Boolean).join(" · ")}
                  </span>
                </div>
                <div className="download-history-overlay">
                  <Button
                    type="button"
                    size="xs"
                    variant="ghost"
                    className="download-history-file-action"
                    onClick={() => void onOpenFolderByJobId(entry.job_id, entry.output_dir)}
                    aria-label={`Open folder for ${entry.course_title}`}
                  >
                    <FolderOpen aria-hidden="true" />
                    Open Folder
                  </Button>
                </div>
              </li>
            );
          })}
        </ol>
      )}
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
    study_guide_total: 0,
    study_guide_completed: 0,
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
  if (job.title?.trim()) return job.title.trim();
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

function jobStatusLabel(status: string, paused = false) {
  if (paused) return "Paused";
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

function serializedStateEqual(left: unknown, right: unknown) {
  if (left === right) return true;
  return JSON.stringify(left) === JSON.stringify(right);
}

async function sleepUntilNextQueueItem(milliseconds: number, shouldStop?: () => boolean) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline && !shouldStop?.()) {
    await sleep(Math.min(500, Math.max(0, deadline - Date.now())));
  }
}

function emptyProcessQueuedDownloadResponse(): ProcessQueuedDownloadResponse {
  return {
    processed: false,
    completed_artifacts: 0,
    failed_artifacts: 0,
    cancelled_artifacts: 0
  };
}

function mergeProcessQueuedDownloadResponses(
  left: ProcessQueuedDownloadResponse,
  right: ProcessQueuedDownloadResponse
): ProcessQueuedDownloadResponse {
  return {
    processed: left.processed || right.processed,
    completed_artifacts: left.completed_artifacts + right.completed_artifacts,
    failed_artifacts: left.failed_artifacts + right.failed_artifacts,
    cancelled_artifacts: left.cancelled_artifacts + right.cancelled_artifacts
  };
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

function isLinkedInSessionError(error: unknown) {
  const message = String(error).toLowerCase();
  return [
    "failed to fetch linkedin learning home",
    "linkedin did not accept this session",
    "linkedin session did not include jsessionid",
    "no valid browser token candidates",
    "csrf check failed",
    "linkedin session expired",
    "rejected the saved session",
    "http 401",
    "http 403",
    "status 401",
    "status 403"
  ].some((marker) => message.includes(marker));
}

async function copyTextToClipboard(value: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    if (!document.execCommand("copy")) {
      throw new Error("Clipboard access is unavailable.");
    }
  } finally {
    textarea.remove();
  }
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

async function requestActiveDownloadCancellation(jobId?: string) {
  if (isTauriRuntime()) {
    return invoke<CancelDownloadResponse>("cancel_active_download");
  }

  const timestamp = Math.floor(Date.now() / 1000);
  const currentJobs = readPreviewJobs();
  const activeJob = currentJobs.find((job) =>
    job.status === "active" && (!jobId || job.id === jobId)
  );
  if (!activeJob) {
    throw new Error("Active download was not found.");
  }
  const jobs = currentJobs.map((job) =>
    job.id === activeJob.id
      ? { ...job, status: "cancelled", paused: false, updated_at: timestamp }
      : job
  );
  const events = [{
    id: Date.now(),
    job_id: activeJob.id,
    event_type: "job.cancelled",
    message: "Download cancelled by user.",
    created_at: timestamp
  }, ...readPreviewEvents()];
  writePreviewState(jobs, events);
  return { cancellation_requested: true } satisfies CancelDownloadResponse;
}

async function setDownloadJobPause(jobId: string, paused: boolean) {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("set_download_job_pause", { jobId, paused });
  }

  const timestamp = Math.floor(Date.now() / 1000);
  const currentJobs = readPreviewJobs();
  const target = currentJobs.find((job) => job.id === jobId);
  if (!target || (target.status !== "active" && target.status !== "queued")) {
    throw new Error("Pausable download was not found.");
  }
  const jobs = currentJobs.map((job) =>
    job.id === jobId ? { ...job, paused, updated_at: timestamp } : job
  );
  const events = [{
    id: Date.now(),
    job_id: jobId,
    event_type: paused ? "job.paused" : "job.resumed",
    message: paused ? "Download paused by user." : "Download resumed by user.",
    created_at: timestamp
  }, ...readPreviewEvents()];
  writePreviewState(jobs, events);
  return previewBootstrapState(jobs, events);
}

async function setAllDownloadsPaused(paused: boolean) {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("set_all_downloads_paused", { paused });
  }

  const timestamp = Math.floor(Date.now() / 1000);
  const currentJobs = readPreviewJobs();
  const changedJobs = currentJobs.filter((job) =>
    (job.status === "active" || job.status === "queued") && Boolean(job.paused) !== paused
  );
  const jobs = currentJobs.map((job) =>
    job.status === "active" || job.status === "queued"
      ? { ...job, paused, updated_at: timestamp }
      : job
  );
  const events = [
    ...changedJobs.map((job, index) => ({
      id: Date.now() * 1000 + index,
      job_id: job.id,
      event_type: paused ? "job.paused" : "job.resumed",
      message: paused ? "Download paused by user." : "Download resumed by user.",
      created_at: timestamp
    })),
    ...readPreviewEvents()
  ];
  writePreviewState(jobs, events);
  return previewBootstrapState(jobs, events);
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
    default_resolution: "P720",
    download_history: downloadHistoryFromJobs(jobs),
    download_history_file_path: previewDownloadHistoryFilePath()
  } satisfies BootstrapState;
}

async function downloadScheduledJobNow(jobId: string) {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("download_scheduled_job_now", { jobId });
  }

  const timestamp = Math.floor(Date.now() / 1000);
  const jobs = readPreviewJobs().map((job) => job.id === jobId ? { ...job, scheduled_at: null, updated_at: timestamp } : job);
  const events = [{
    id: timestamp,
    job_id: jobId,
    event_type: "job.schedule.override",
    message: "Scheduled course was moved to the immediate download queue.",
    created_at: timestamp
  }, ...readPreviewEvents()];
  writePreviewState(jobs, events);
  return previewBootstrapState(jobs, events);
}

function previewBootstrapState(jobs: QueuedDownloadJob[], events: PersistedJobEvent[]): BootstrapState {
  return {
    default_resolution: "P720",
    browser_sources: ["Chrome", "Edge", "Firefox"],
    stores_plaintext_tokens_in_sqlite: false,
    has_saved_token: hasPreviewSavedToken(),
    saved_download_preferences: readPreviewPreferences(),
    persisted_jobs: jobs,
    recent_events: events,
    download_history: downloadHistoryFromJobs(jobs),
    download_history_file_path: previewDownloadHistoryFilePath()
  };
}

async function removeDownloadQueueItem(jobId: string) {
  if (isTauriRuntime()) {
    return invoke<BootstrapState>("remove_download_queue_item", { jobId });
  }

  const jobs = readPreviewJobs().filter((job) => job.id !== jobId || job.status === "active");
  const events = readPreviewEvents().filter((event) => event.job_id !== jobId);
  writePreviewState(jobs, events);
  return {
    persisted_jobs: jobs,
    recent_events: events,
    has_saved_token: hasPreviewSavedToken(),
    saved_download_preferences: readPreviewPreferences(),
    stores_plaintext_tokens_in_sqlite: false,
    browser_sources: ["Chrome", "Edge", "Firefox"],
    default_resolution: "P720",
    download_history: downloadHistoryFromJobs(jobs),
    download_history_file_path: previewDownloadHistoryFilePath()
  } satisfies BootstrapState;
}

async function checkForAppUpdate() {
  if (isTauriRuntime()) {
    return invoke<UpdateMetadata | null>("check_for_app_update");
  }

  return null;
}

async function installAppUpdate() {
  if (isTauriRuntime()) {
    await invoke("install_app_update");
    return true;
  }

  guardedToast("Updater unavailable in preview", "Installers can only update inside the packaged desktop app.");
  return false;
}

async function openDownloadFolder(jobId: string, previewPath?: string) {
  if (isTauriRuntime()) {
    return invoke<{ path: string }>("open_download_folder", { jobId });
  }

  guardedToast("Folder opener unavailable in preview", previewPath || jobId);
  return null;
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

async function processNextQueuedDownloadWithBrowserSource(source: string) {
  if (isTauriRuntime()) {
    return invoke<ProcessQueuedDownloadResponse>("process_next_queued_download_from_browser_source", { source });
  }

  throw new Error("Browser session downloads are only available in the desktop app");
}

function parseLinkedInCourseUrlsForPreview(input: string): ParsedCourse[] {
  const courses: ParsedCourse[] = [];
  for (const [index, rawLine] of input.split(/\r?\n/).entries()) {
    const line = index + 1;
    const candidates = courseUrlCandidatesForPreview(rawLine);
    if (candidates.length === 0) {
      if (!rawLine.trim()) continue;
      throw previewCourseUrlErrorMessage({ type: "notLinkedInLearning", line });
    }
    courses.push(...candidates.map((candidate) => parseLinkedInCourseUrlForPreview(candidate, line)));
  }

  if (courses.length === 0) {
    throw previewCourseUrlErrorMessage({ type: "empty" });
  }

  return courses;
}

function courseUrlCandidatesForPreview(line: string): string[] {
  const trimmed = line.trim();
  if (!trimmed) return [];
  const parts = trimmed.split(/\s+/);
  if (parts.length === 1) return [trimCourseUrlTokenForPreview(parts[0])];
  return parts
    .map(trimCourseUrlTokenForPreview)
    .filter((part) => part.toLowerCase().includes("linkedin.com/learning/"));
}

function trimCourseUrlTokenForPreview(token: string) {
  return token.replace(/^[\s"'`<({\[]+|[\s"'`,>)}\]]+$/g, "");
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
  const requestId = Date.now();
  const scheduledTimes = previewScheduledTimes(request.schedule, parsed.length, timestamp);
  const jobs = parsed.map((course, index) => ({
    id: `preview-job-${requestId}-${index + 1}-${course.slug}`,
    course_slug: course.slug,
    source_url: course.normalized_url,
    status: "queued",
    thumbnail_url: previewThumbnailForSlug(course.slug),
    selected_quality: request.selectedQuality,
    output_dir: request.outputDir,
    paused: false,
    scheduled_at: scheduledTimes[index],
    updated_at: timestamp,
    artifact_counts: emptyArtifactCounts()
  }));

  writePreviewState([...jobs, ...readPreviewJobs()], readPreviewEvents());
  return { jobs };
}

function previewScheduledTimes(schedule: DownloadScheduleRequest | undefined, courseCount: number, timestamp: number) {
  if (!schedule) return Array.from({ length: courseCount }, () => null as number | null);
  const windowMinutes = schedule.windowMinutes;
  let elapsedMinutes = 0;
  return Array.from({ length: courseCount }, (_, index) => {
    const remainingCourses = courseCount - index - 1;
    const available = Math.max(schedule.minWaitMinutes, windowMinutes - elapsedMinutes - remainingCourses * schedule.minWaitMinutes);
    const maxWait = Math.min(schedule.maxWaitMinutes, available);
    const range = Math.max(1, maxWait - schedule.minWaitMinutes + 1);
    const wait = schedule.minWaitMinutes + ((timestamp + index * 17) % range);
    elapsedMinutes += wait;
    return timestamp + elapsedMinutes * 60;
  });
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
    recent_events: events,
    download_history: downloadHistoryFromJobs(retriedJobs),
    download_history_file_path: previewDownloadHistoryFilePath()
  };
}

async function processNextQueuedDownloadForPreview(): Promise<ProcessQueuedDownloadResponse> {
  const jobs = readPreviewJobs();
  const scenario = getPreviewScenario();
  const readyAt = Math.floor(Date.now() / 1000);
  const queuedIndex = jobs.findIndex((job) => job.status === "queued" && !job.paused && (!job.scheduled_at || job.scheduled_at <= readyAt));
  if (queuedIndex < 0) {
    return {
      processed: false,
      completed_artifacts: 0,
      failed_artifacts: 0,
      cancelled_artifacts: 0
    };
  }

  if (scenario === "live-polling-progress") {
    return processLivePollingProgressForPreview(jobs, queuedIndex);
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

    const completedJob = {
      ...jobs[queuedIndex],
      status: "completed",
      updated_at: timestamp,
      artifact_counts: {
        total: 6,
        completed: 6,
        failed: 0,
        cancelled: 0,
        active: 0,
        pending: 0,
        skipped: 0,
        video_total: 3,
        video_completed: 3,
        subtitle_total: 2,
        subtitle_completed: 2,
        exercise_total: 1,
        exercise_completed: 1
      }
    };
    const nextJobs = jobs.map((job, index) => (index === queuedIndex ? completedJob : job));
    writePreviewState(nextJobs, [
        {
          id: 1,
          job_id: completedJob.id,
          event_type: "job.completed",
          message: "Completed one queued course before continuing to the next course.",
          created_at: timestamp
        },
        {
          id: 2,
          job_id: completedJob.id,
          event_type: "artifact.completed",
          message: "Course video, subtitle, and exercise artifacts completed.",
          created_at: timestamp - 1
        }
    ]);

    return {
      processed: true,
      completed_artifacts: 6,
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

    throw new Error("Course metadata fetch or artifact planning failed.");
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

  const timestamp = Math.floor(Date.now() / 1000);
  const completedJob = {
    ...jobs[queuedIndex],
    status: "completed",
    updated_at: timestamp,
    artifact_counts: {
      total: 1,
      completed: 1,
      failed: 0,
      cancelled: 0,
      active: 0,
      pending: 0,
      skipped: 0,
      video_total: 1,
      video_completed: 1,
      subtitle_total: 0,
      subtitle_completed: 0,
      exercise_total: 0,
      exercise_completed: 0
    }
  };
  writePreviewState(jobs.map((job, index) => (index === queuedIndex ? completedJob : job)), [
    {
      id: timestamp,
      job_id: completedJob.id,
      event_type: "job.completed",
      message: "Preview queued course completed.",
      created_at: timestamp
    }
  ]);

  return {
    processed: true,
    completed_artifacts: 1,
    failed_artifacts: 0,
    cancelled_artifacts: 0
  };
}

async function processLivePollingProgressForPreview(jobs: QueuedDownloadJob[], queuedIndex: number): Promise<ProcessQueuedDownloadResponse> {
  const timestamp = Math.floor(Date.now() / 1000);
  const unsafeStreamingToken = "do-not-render-live-polling-token";
  void unsafeStreamingToken;

  const activeJob = {
    ...jobs[queuedIndex],
    status: "active",
    updated_at: timestamp,
    video_artifacts: [
      previewVideoArtifact("preview-video-1", "01-course-introduction.mp4", "completed", timestamp - 2, 18_400_000),
      previewVideoArtifact("preview-video-2", "02-building-the-first-workflow.mp4", "pending", timestamp),
      previewVideoArtifact("preview-video-3", "03-review-and-next-steps.mp4", "pending", timestamp)
    ],
    artifact_counts: {
      total: 6,
      completed: 1,
      failed: 0,
      cancelled: 0,
      active: 0,
      pending: 5,
      skipped: 0,
      video_total: 3,
      video_completed: 1,
      subtitle_total: 2,
      subtitle_completed: 0,
      exercise_total: 1,
      exercise_completed: 0
    }
  };
  writePreviewState(jobs.map((job, index) => (index === queuedIndex ? activeJob : job)), [
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
      event_type: "video.pacing.wait",
      message: "Waiting 20 seconds before the next video request.",
      payload_json: JSON.stringify({
        artifactId: "preview-video-2",
        waitSeconds: 20,
        waitStartedAt: timestamp,
        waitUntil: timestamp + 20
      }),
      created_at: timestamp
    }
  ]);

  await sleep(5_000);

  const updatedActiveJob = {
    ...activeJob,
    updated_at: timestamp + 1,
    video_artifacts: activeJob.video_artifacts.map((artifact) =>
      artifact.id === "preview-video-2" ? { ...artifact, status: "active", updated_at: timestamp + 5 } : artifact
    ),
    artifact_counts: {
      ...activeJob.artifact_counts,
      completed: 2,
      active: 1,
      pending: 3,
      video_completed: 1,
      subtitle_completed: 1
    }
  };
  writePreviewState(jobs.map((job, index) => (index === queuedIndex ? updatedActiveJob : job)), [
    {
      id: 3,
      job_id: activeJob.id,
      event_type: "artifact.active",
      message: "Live polling course video started.",
      created_at: timestamp + 5
    },
    {
      id: 2,
      job_id: activeJob.id,
      event_type: "artifact.active",
      message: "Live polling course subtitles started.",
      created_at: timestamp
    }
  ]);

  await sleep(1_500);

  const completedJob = {
    ...updatedActiveJob,
    status: "completed",
    updated_at: timestamp + 2,
    video_artifacts: updatedActiveJob.video_artifacts.map((artifact, index) => ({
      ...artifact,
      status: "completed",
      size_bytes: artifact.size_bytes ?? 22_000_000 + index * 4_000_000,
      updated_at: timestamp + 7 + index
    })),
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
  writePreviewState(jobs.map((job, index) => (index === queuedIndex ? completedJob : job)), [
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

function previewVideoArtifact(
  id: string,
  displayName: string,
  status: string,
  timestamp: number,
  sizeBytes?: number
): VideoDownloadArtifact {
  return {
    id,
    display_name: displayName,
    status,
    size_bytes: sizeBytes ?? null,
    created_at: timestamp,
    updated_at: timestamp
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

function downloadHistoryFromJobs(jobs: QueuedDownloadJob[]): DownloadHistoryEntry[] {
  return jobs
    .filter((job) => job.status === "completed")
    .map((job) => ({
      job_id: job.id,
      course_slug: job.course_slug,
      source_url: job.source_url || `https://www.linkedin.com/learning/${job.course_slug}`,
      course_title: courseDisplayName(job),
      output_dir: job.output_dir || "",
      completed_at: job.updated_at ?? 0
    }))
    .sort((left, right) => right.completed_at - left.completed_at);
}

function previewDownloadHistoryFilePath() {
  return "Preview/LinkVaultData/download-history.md";
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
