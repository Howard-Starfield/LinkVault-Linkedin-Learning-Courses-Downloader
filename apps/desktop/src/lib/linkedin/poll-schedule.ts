/**
 * Decides when the LinkedIn queue poll should run again.
 *
 * The poll is not only a UI refresh. `checkDueSchedules` in App.tsx also calls
 * `ensureDownloadProcessing`, and that is what actually starts a scheduled
 * download once its `scheduled_at` passes. So polling must keep running while
 * any job is `active` or `queued` -- a future-scheduled job is `queued`, which
 * is exactly why `queued` cannot be excluded from the gate.
 *
 * Returning `null` means "nothing can change on its own; stop polling". The
 * caller re-arms on any user action, so the view still updates.
 */

/** The subset of a queued job this decision actually needs. */
export type PollableJob = {
  status: string;
  paused?: boolean;
  scheduled_at?: number | null;
};

/** While a download is running we poll for progress and video pacing. */
export const ACTIVE_POLL_INTERVAL_MS = 15_000;

/**
 * Upper bound on any single sleep. This is the backstop, and it is deliberately
 * much longer than a typical schedule: within an hour a scheduled job sleeps
 * exactly until it is due, so a course queued for 20 minutes from now costs one
 * wake-up rather than eighty. Beyond that the poll re-checks hourly, which keeps
 * the worst case bounded if a wake-up is ever missed or the machine was
 * suspended, while an install with one pending job still only polls 24x a day.
 */
export const IDLE_POLL_CEILING_MS = 60 * 60_000;

/** Never re-arm faster than this, so a job due in 200ms cannot spin the loop. */
const MIN_POLL_DELAY_MS = 1_000;

function isDueLater(
  scheduledAt: number | null | undefined,
  nowMs: number
): scheduledAt is number {
  return typeof scheduledAt === "number" && scheduledAt * 1000 > nowMs;
}

export function nextPollDelayMs(jobs: readonly PollableJob[], nowMs: number): number | null {
  // A running download needs the fast cadence for progress and pacing.
  if (jobs.some((job) => job.status === "active")) {
    return ACTIVE_POLL_INTERVAL_MS;
  }

  const pending = jobs.filter((job) => job.status === "queued");
  if (pending.length === 0) {
    // Only completed/failed jobs remain. Nothing progresses without a user
    // action, and every user action re-arms the poll.
    return null;
  }

  // A queued job that is already runnable is starting now; keep the cadence
  // until it flips to `active` (or fails).
  if (pending.some((job) => !job.paused && !isDueLater(job.scheduled_at, nowMs))) {
    return ACTIVE_POLL_INTERVAL_MS;
  }

  // Everything queued is paused or scheduled for later. Sleep until the earliest
  // unpaused one comes due, clamped to the floor and the ceiling.
  let nextDueMs: number | null = null;
  for (const job of pending) {
    if (job.paused) continue;
    const scheduledAt = job.scheduled_at;
    if (!isDueLater(scheduledAt, nowMs)) continue;
    const dueMs = scheduledAt * 1000;
    if (nextDueMs === null || dueMs < nextDueMs) {
      nextDueMs = dueMs;
    }
  }

  if (nextDueMs === null) {
    // Everything queued is paused; only a user action changes that.
    return IDLE_POLL_CEILING_MS;
  }

  const waitMs = nextDueMs - nowMs;
  return Math.max(MIN_POLL_DELAY_MS, Math.min(waitMs, IDLE_POLL_CEILING_MS));
}
