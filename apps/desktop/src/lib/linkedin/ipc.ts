import { invoke } from "@tauri-apps/api/core";
import type {
  CatalogEntry,
  CoursePlayback,
  CourseSlug,
  PathSlug,
  PlaybackTick,
  VideoProgress
} from "./types";

export type LinkedInDestinationCommit<TBootstrap> = {
  outputDir: string;
  imported: number;
  skipped: number;
  alreadyKnown: number;
  bootstrap: TBootstrap;
};

export async function commitLinkedInDestination<TBootstrap>(
  path: string
): Promise<LinkedInDestinationCommit<TBootstrap>> {
  return invoke<LinkedInDestinationCommit<TBootstrap>>("commit_linkedin_destination", { path });
}

export async function linkedinListCatalog(): Promise<CatalogEntry[]> {
  return invoke<CatalogEntry[]>("linkedin_list_catalog");
}

export async function linkedinOpenCourse(courseSlug: CourseSlug): Promise<CoursePlayback> {
  return invoke<CoursePlayback>("linkedin_open_course", { courseSlug });
}

export async function linkedinSaveProgress(tick: PlaybackTick): Promise<VideoProgress> {
  return invoke<VideoProgress>("linkedin_save_progress", { tick });
}

export async function linkedinOpenCourseFolder(courseSlug: CourseSlug): Promise<void> {
  return invoke<void>("linkedin_open_course_folder", { courseSlug });
}

export async function linkedinAddCourseToPath(
  courseSlug: CourseSlug,
  pathSlug: PathSlug
): Promise<CatalogEntry[]> {
  return invoke<CatalogEntry[]>("linkedin_add_course_to_path", { courseSlug, pathSlug });
}
