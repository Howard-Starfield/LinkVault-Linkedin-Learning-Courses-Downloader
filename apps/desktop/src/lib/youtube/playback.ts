import type { YouTubePlaybackProgress, YouTubeRunVideo } from "./types";

/** Playback position is persisted at most this often while a video plays. */
export const PROGRESS_SAVE_INTERVAL_MS = 5_000;
/** A forced save (pause, end, close) is skipped when the position moved less than this. */
const FORCED_SAVE_MIN_DELTA_SECONDS = 0.5;
/** Resume restarts from 0 when this close to the end. */
const RESUME_END_GUARD_SECONDS = 3;

export interface PlaybackSample {
  occurrenceId: string;
  positionSeconds: number;
  durationSeconds: number;
}

export function isSavableSample(sample: PlaybackSample): boolean {
  return (
    sample.occurrenceId.length > 0 &&
    Number.isFinite(sample.positionSeconds) &&
    Number.isFinite(sample.durationSeconds) &&
    sample.positionSeconds >= 0 &&
    sample.durationSeconds > 0
  );
}

/**
 * Decides when playback progress is worth persisting.
 *
 * While playing, a sample is saved at most every `intervalMs`. Forced samples
 * (pause, ended, switching video, closing) bypass the interval but are still
 * dropped when nothing moved since the last save, so closing right after a
 * pause does not write twice. The caller calls `markSaved` for every sample it
 * actually sends, so a sample the throttle declined never starts the interval.
 */
export function createProgressThrottle(intervalMs: number = PROGRESS_SAVE_INTERVAL_MS) {
  let lastSavedAt = Number.NEGATIVE_INFINITY;
  let lastSaved: PlaybackSample | null = null;
  return {
    shouldSave(sample: PlaybackSample, nowMs: number, force: boolean): boolean {
      if (!isSavableSample(sample)) return false;
      if (force) {
        return !(
          lastSaved !== null &&
          lastSaved.occurrenceId === sample.occurrenceId &&
          Math.abs(lastSaved.positionSeconds - sample.positionSeconds) < FORCED_SAVE_MIN_DELTA_SECONDS
        );
      }
      return nowMs - lastSavedAt >= intervalMs;
    },
    markSaved(sample: PlaybackSample, nowMs: number): void {
      lastSavedAt = nowMs;
      lastSaved = sample;
    }
  };
}

/**
 * Mirrors the backend rule that marks a video finished: within the last 5% of
 * the video, but never more than 10 seconds from the end.
 */
export function isPlaybackCompleted(positionSeconds: number, durationSeconds: number): boolean {
  if (!Number.isFinite(positionSeconds) || !Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    return false;
  }
  return durationSeconds - positionSeconds <= Math.min(durationSeconds * 0.05, 10);
}

/** Where to start a video: the saved position, unless it is unset, finished, or at the very end. */
export function resumePositionSeconds(
  progress: Pick<YouTubePlaybackProgress, "positionSeconds" | "durationSeconds" | "completed"> | null
): number {
  if (!progress || progress.completed) return 0;
  const { positionSeconds, durationSeconds } = progress;
  if (!Number.isFinite(positionSeconds) || positionSeconds <= 0) return 0;
  if (Number.isFinite(durationSeconds) && durationSeconds > 0 && positionSeconds >= durationSeconds - RESUME_END_GUARD_SECONDS) {
    return 0;
  }
  return positionSeconds;
}

export function progressOfVideo(video: YouTubeRunVideo): YouTubePlaybackProgress | null {
  if (video.positionSeconds === null) return null;
  return {
    positionSeconds: video.positionSeconds,
    durationSeconds: video.durationSeconds ?? 0,
    completed: video.completed
  };
}

/** The next (direction 1) or previous (-1) playable video in run order, or null at either end. */
export function adjacentPlayableVideo(
  videos: readonly YouTubeRunVideo[],
  currentOccurrenceId: string,
  direction: 1 | -1
): YouTubeRunVideo | null {
  const start = videos.findIndex((video) => video.occurrenceId === currentOccurrenceId);
  if (start < 0) return null;
  for (let index = start + direction; index >= 0 && index < videos.length; index += direction) {
    const candidate = videos[index];
    if (candidate && candidate.available && candidate.mediaUrl) return candidate;
  }
  return null;
}

export function playableVideos(videos: readonly YouTubeRunVideo[]): YouTubeRunVideo[] {
  return videos.filter((video) => video.available && video.mediaUrl !== null);
}

export function formatPlaybackClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  const tail = `${minutes.toString().padStart(hours > 0 ? 2 : 1, "0")}:${rest.toString().padStart(2, "0")}`;
  return hours > 0 ? `${hours}:${tail}` : tail;
}

/** Short status for a video row: where it will resume, or that it was finished. */
export function describeVideoProgress(video: YouTubeRunVideo): string | null {
  if (video.completed) return "Watched";
  const resume = resumePositionSeconds(progressOfVideo(video));
  return resume > 0 ? `Resume at ${formatPlaybackClock(resume)}` : null;
}

/**
 * Runs saves one after another so two quick saves (for example a pause and a
 * close) reach the writer in the order they were made.
 */
export function createSerialRunner() {
  let tail: Promise<unknown> = Promise.resolve();
  return {
    run<T>(task: () => Promise<T>): Promise<T> {
      const result = tail.then(task, task);
      tail = result.catch(() => undefined);
      return result;
    }
  };
}
