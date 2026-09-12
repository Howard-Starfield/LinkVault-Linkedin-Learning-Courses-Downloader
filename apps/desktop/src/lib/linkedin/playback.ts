import type { CoursePlayback, VideoPlayback, VideoProgress, VideoSlug } from "./types";

const COMPLETION_TAIL_MS = 3000;
const COMPLETION_TAIL_NUMERATOR = 2;
const COMPLETION_TAIL_DENOMINATOR = 100;
const COMPLETION_TAIL_FLOOR_MS = 500;

export function playableCourseVideos(playback: CoursePlayback): VideoPlayback[] {
  return playback.chapters.flatMap((chapter) =>
    chapter.videos.filter((video) => video.mediaUrl)
  );
}

export function completionThresholdMs(durationMs: number): number {
  if (!(durationMs > 0)) return COMPLETION_TAIL_FLOOR_MS;
  const percent = (durationMs * COMPLETION_TAIL_NUMERATOR) / COMPLETION_TAIL_DENOMINATOR;
  return Math.min(COMPLETION_TAIL_MS, Math.max(COMPLETION_TAIL_FLOOR_MS, percent));
}

export function resumePlaybackMs(progress: VideoProgress): number {
  const position = Number.isFinite(progress.positionMs) ? Math.max(0, progress.positionMs) : 0;
  const duration = Number.isFinite(progress.durationMs) ? Math.max(0, progress.durationMs) : 0;
  if (progress.completedAt !== null) return 0;
  if (duration > 0 && position >= duration - completionThresholdMs(duration)) return 0;
  return position;
}

export function nextPlayableVideo(
  playback: CoursePlayback,
  current: VideoSlug
): VideoPlayback | null {
  const playable = playableCourseVideos(playback);
  const index = playable.findIndex((video) => video.video === current);
  if (index < 0 || index + 1 >= playable.length) {
    return null;
  }
  return playable[index + 1];
}
