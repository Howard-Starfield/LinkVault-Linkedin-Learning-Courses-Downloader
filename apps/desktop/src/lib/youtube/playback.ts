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
 * pause does not write twice. `markSaved` is called only after a write
 * succeeded, so a failed save stays dirty and the next save or the close
 * retries it.
 */
export function createProgressThrottle(intervalMs: number = PROGRESS_SAVE_INTERVAL_MS) {
  let lastSavedAt = Number.NEGATIVE_INFINITY;
  let lastAttemptAt = Number.NEGATIVE_INFINITY;
  let lastRequested: PlaybackSample | null = null;
  let lastRequestFailed = false;
  return {
    shouldSave(sample: PlaybackSample, nowMs: number, force: boolean): boolean {
      if (!isSavableSample(sample)) return false;
      if (force) {
        // Skip only a repeat of the last value that was *requested* (queued,
        // in flight, or done). Comparing with the last committed value would
        // drop a pause at 10 while a save of 20 is still pending, and 20
        // would win.
        return !(
          lastRequested !== null &&
          !lastRequestFailed &&
          lastRequested.occurrenceId === sample.occurrenceId &&
          Math.abs(lastRequested.positionSeconds - sample.positionSeconds) < FORCED_SAVE_MIN_DELTA_SECONDS
        );
      }
      // A failed attempt waits one interval before the next tick retries, so
      // a persistent failure is not hammered four times a second.
      return nowMs - lastSavedAt >= intervalMs && nowMs - lastAttemptAt >= intervalMs;
    },
    /** Call when a save is queued. Does not count as saved until `markSaved`. */
    markAttempted(sample: PlaybackSample, nowMs: number): void {
      lastAttemptAt = nowMs;
      lastRequested = sample;
      lastRequestFailed = false;
    },
    /** Call only after the save succeeded. */
    markSaved(nowMs: number): void {
      lastSavedAt = nowMs;
    },
    /** Call when a save failed: its value stays dirty and is retried. */
    markFailed(sample: PlaybackSample): void {
      if (lastRequested === sample) lastRequestFailed = true;
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
    /** Resolves once every save queued so far has finished (successfully or not). */
    drain(): Promise<void> {
      return tail.then(() => undefined);
    },
    run<T>(task: () => Promise<T>): Promise<T> {
      const result = tail.then(task, task);
      tail = result.catch(() => undefined);
      return result;
    }
  };
}

export type SerialRunner = ReturnType<typeof createSerialRunner>;

/**
 * Throttled, ordered, retrying progress saves for one video.
 *
 * - `request` saves when the throttle allows (or `force`) and always resolves
 *   only after the shared queue has drained, so a close can await it and the
 *   history reload never sees stale progress, even when nothing new was sent.
 * - A save counts as done only after it succeeded; on failure the error is
 *   reported and the position stays dirty for the next save or the close.
 */
export function createProgressSaver(options: {
  runner: SerialRunner;
  save: (sample: PlaybackSample) => Promise<unknown>;
  now?: () => number;
  intervalMs?: number;
  onError?: (error: unknown) => void;
  /** True once the session started closing: ordinary saves become no-ops. */
  isClosing?: () => boolean;
}) {
  let lastCommitted: PlaybackSample | null = null;
  const commit = (sample: PlaybackSample) => {
    lastCommitted = sample;
  };
  const throttle = createProgressThrottle(options.intervalMs);
  const clock = options.now ?? (() => Date.now());
  return {
    request(sample: PlaybackSample, force: boolean): Promise<void> {
      const now = clock();
      if (options.isClosing?.()) return options.runner.drain();
      if (!throttle.shouldSave(sample, now, force)) return options.runner.drain();
      throttle.markAttempted(sample, now);
      return options.runner
        .run(() => options.save(sample))
        .then(
          () => {
            throttle.markSaved(now);
            commit(sample);
          },
          (error: unknown) => {
            throttle.markFailed(sample);
            options.onError?.(error);
          }
        )
        .then(() => options.runner.drain());
    },
    /**
     * The close save: wait for every queued save first, then, if this exact
     * position did not commit (it failed, or was never sent), save it once
     * more. Works even while the session is closing.
     */
    async finalize(sample: PlaybackSample): Promise<void> {
      await options.runner.drain();
      if (!isSavableSample(sample)) return;
      if (
        lastCommitted !== null &&
        lastCommitted.occurrenceId === sample.occurrenceId &&
        Math.abs(lastCommitted.positionSeconds - sample.positionSeconds) < FORCED_SAVE_MIN_DELTA_SECONDS
      ) {
        return;
      }
      try {
        await options.runner.run(() => options.save(sample));
        commit(sample);
      } catch (error) {
        options.onError?.(error);
      }
    }
  };
}

/**
 * State shared by every video of one player session: the single save queue
 * (so a switch-away save and the close save are serialized together) and the
 * closing flag. After `beginClose`, navigation and ordinary saves must stop.
 */
export function createPlaybackSession() {
  const runner = createSerialRunner();
  let closing = false;
  return {
    runner,
    isClosing: () => closing,
    canNavigate: () => !closing,
    /** Returns false when a close is already under way. */
    beginClose(): boolean {
      if (closing) return false;
      closing = true;
      return true;
    }
  };
}

/**
 * Decides whether "the video ended" may still advance to the next one once
 * the ended-save has finished. Every earlier pending advance is invalidated by
 * `invalidate`, which the player calls on play, seeking, source change,
 * close and unmount; the advance also requires the media to still be ended.
 */
export function createEndedAdvance(options: { onAdvance: () => void }) {
  let generation = 0;
  return {
    invalidate(): void {
      generation += 1;
    },
    afterSave(save: Promise<unknown>, isStillEnded: () => boolean): Promise<void> {
      const captured = generation;
      return save.then(() => {
        if (generation === captured && isStillEnded()) options.onAdvance();
      });
    }
  };
}

export type PlaybackSession = ReturnType<typeof createPlaybackSession>;
