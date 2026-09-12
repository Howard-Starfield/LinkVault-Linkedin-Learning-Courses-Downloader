export type PathSlug = string & { readonly __brand: "PathSlug" };
export type CourseSlug = string & { readonly __brand: "CourseSlug" };
export type VideoSlug = string & { readonly __brand: "VideoSlug" };
export type LinkedinMediaUrl = string & { readonly __brand: "LinkedinMediaUrl" };

export type PlayerSession = {
  course: CourseSlug;
  video: VideoSlug;
} | null;

export type CourseVia =
  | { kind: "path"; path: PathSlug }
  | { kind: "standalone" };

export type LibraryNav =
  | { level: "catalog" }
  | { level: "path"; path: PathSlug }
  | { level: "course"; course: CourseSlug; via: CourseVia };

export type NavAction =
  | { type: "openPath"; path: PathSlug }
  | { type: "openCourse"; course: CourseSlug; via: CourseVia }
  | { type: "back" };

export type JobStatusChip = "queued" | "downloading" | "completed" | "failed" | "absent";

export type PathCourseSummary = {
  course: CourseSlug;
  title: string;
  thumbnailUrl: string | null;
  completedVideos: number;
  totalVideos: number;
  jobStatus: JobStatusChip;
};

export type PathCatalogEntry = {
  kind: "path";
  path: PathSlug;
  title: string;
  sourceUrl: string;
  completedVideos: number;
  totalVideos: number;
  updatedAt: number;
  courses: PathCourseSummary[];
};

export type StandaloneCatalogEntry = {
  kind: "standalone";
  course: CourseSlug;
  title: string;
  sourceUrl: string;
  thumbnailUrl: string | null;
  completedVideos: number;
  totalVideos: number;
  updatedAt: number;
};

export type CatalogEntry = PathCatalogEntry | StandaloneCatalogEntry;

export type VideoProgress = {
  positionMs: number;
  durationMs: number;
  completedAt: number | null;
};

export type VideoPlayback = {
  video: VideoSlug;
  title: string;
  durationMs: number;
  progress: VideoProgress;
  mediaUrl: LinkedinMediaUrl | null;
};

export type ChapterPlayback = {
  title: string;
  videos: VideoPlayback[];
};

export type CoursePlayback = {
  course: CourseSlug;
  title: string;
  chapters: ChapterPlayback[];
};

export type PlaybackTick = {
  course: CourseSlug;
  video: VideoSlug;
  positionMs: number;
  durationMs: number;
};

export function asPathSlug(value: string): PathSlug {
  return value as PathSlug;
}

export function asCourseSlug(value: string): CourseSlug {
  return value as CourseSlug;
}

export function asVideoSlug(value: string): VideoSlug {
  return value as VideoSlug;
}
