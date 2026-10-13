import type { YouTubeItemPhase, YouTubeItemState, YouTubeResumableRun } from "./types";

/** Where a result row sits relative to a download run. */
export type YouTubeRowRunContext = "idle" | "active" | "terminal";

/** Stage text for a running item, from the phase the backend last reported. */
export function youtubeStageLabel(phase: YouTubeItemPhase): string {
  switch (phase) {
    case "waiting": return "Fetching video…";
    case "media": return "Downloading video…";
    case "transcript": return "Downloading transcript…";
    case "normalizing_transcript": return "Processing transcript…";
    case "verifying": return "Checking media…";
    case "merging": return "Merging…";
    case "finalizing": return "Saving…";
    // Terminal phases can only be seen on a running row for a moment between updates.
    default: return "Working…";
  }
}

/** Bytes the backend reported for the item that is downloading right now. */
export interface YouTubeByteProgress {
  bytesCompleted: number | null;
  bytesTotal: number | null;
}

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;
/** Non-breaking space, so a narrow row wraps between segments, never inside a byte count. */
const NBSP = "\u00A0";

/** The value as shown: whole bytes, otherwise one decimal. */
function roundedBytes(bytes: number, unitIndex: number): number {
  const value = bytes / 1000 ** unitIndex;
  return unitIndex === 0 ? Math.round(value) : Math.round(value * 10) / 10;
}

/** Largest unit that keeps the shown value below 1000 (999,950 B is "1.0 MB", not "1000.0 KB"). */
function byteUnitIndex(bytes: number): number {
  let index = 0;
  while (index < BYTE_UNITS.length - 1 && roundedBytes(bytes, index) >= 1000) index += 1;
  return index;
}

function scaledBytes(bytes: number, unitIndex: number): string {
  const value = roundedBytes(bytes, unitIndex);
  return unitIndex === 0 ? String(value) : value.toFixed(1);
}

function bytesWithUnit(bytes: number, unitIndex: number): string {
  return `${scaledBytes(bytes, unitIndex)}${NBSP}${BYTE_UNITS[unitIndex]}`;
}

/**
 * "18.3 / 43.6 MB" (decimal units, both numbers in the total's unit), or
 * "18.3 MB" while the total is unknown. When the completed amount is too small
 * to show in the total's unit (20 KB of 3 GB) it keeps its own: "20.0 KB / 3.0 GB".
 * Returns null when there is nothing meaningful to show.
 */
export function formatYouTubeBytes(progress: YouTubeByteProgress | null): string | null {
  if (!progress) return null;
  const { bytesCompleted, bytesTotal } = progress;
  if (bytesCompleted === null || !Number.isFinite(bytesCompleted) || bytesCompleted <= 0) return null;
  const hasTotal = bytesTotal !== null && Number.isFinite(bytesTotal) && bytesTotal > 0;
  if (!hasTotal) return bytesWithUnit(bytesCompleted, byteUnitIndex(bytesCompleted));
  const unitIndex = byteUnitIndex(bytesTotal);
  const completed = Math.min(bytesCompleted, bytesTotal);
  const total = bytesWithUnit(bytesTotal, unitIndex);
  if (roundedBytes(completed, unitIndex) === 0) {
    return [bytesWithUnit(completed, byteUnitIndex(completed)), "/", total].join(NBSP);
  }
  return [scaledBytes(completed, unitIndex), "/", total].join(NBSP);
}

/**
 * Percent and, when known, bytes: "42% · 18.3 / 43.6 MB". Kept apart from the
 * stage label so only the label sits in a live region and screen readers are
 * not told about every progress tick.
 */
export function youtubeProgressDetail(percent: number, bytes: YouTubeByteProgress | null): string {
  const parts: string[] = [];
  if (percent > 0) parts.push(`${Math.round(percent)}%`);
  const byteText = formatYouTubeBytes(bytes);
  if (byteText) parts.push(byteText);
  return parts.join(" · ");
}

/** Stage text plus percent and, when known, bytes: "Downloading video… · 42% · 18.3 / 43.6 MB". */
export function youtubeProgressText(
  status: string | null,
  percent: number,
  bytes: YouTubeByteProgress | null
): string {
  return [status ?? "", youtubeProgressDetail(percent, bytes)].filter((part) => part !== "").join(" · ");
}

/**
 * Short status line for a result row. A pending item is "Queued" while its run
 * is active and "Not started" once the run ended without reaching it; a video
 * that is not part of any run has no status.
 */
export function youtubeItemStatusText(
  state: YouTubeItemState,
  phase: YouTubeItemPhase | null,
  available: boolean,
  runContext: YouTubeRowRunContext
): string | null {
  if (!available) return state === "skipped" ? "Unavailable" : "Unconfirmed";
  switch (state) {
    case "completed": return "Saved";
    case "completed_with_warnings": return "Saved with warnings";
    case "running": return youtubeStageLabel(phase ?? "waiting");
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    case "skipped_existing": return "Already saved";
    case "pending":
      if (runContext === "active") return "Queued";
      if (runContext === "terminal") return "Not started";
      return null;
    default: return null;
  }
}

/** Live byte progress of a run in a resumed attempt, when the backend has reported any. */
export type ResumeProgress = YouTubeByteProgress | null;

/** The Continue button applies while the run waits for a decision, never while it runs. */
export function canContinueResumable(run: YouTubeResumableRun): boolean {
  return run.state === "needs_user" || run.state === "auto_pending";
}

/**
 * One status line per run:
 * - waiting for the automatic resume: "Interrupted — resuming automatically…"
 * - waiting for the user: "Paused — <reason>"
 * - resumed attempt: "Resuming from 120.0 MB" before yt-dlp reports, then "Resuming — 130.5 / 1.2 GB"
 */
export function resumableStatusText(run: YouTubeResumableRun, progress: ResumeProgress): string {
  switch (run.state) {
    case "auto_pending":
      return "Interrupted — resuming automatically…";
    case "needs_user": {
      const reason = run.reason?.trim();
      return reason ? `Paused — ${reason}` : "Paused — press Continue to pick up where it stopped.";
    }
    case "resuming": {
      const bytes = formatYouTubeBytes(progress);
      if (bytes) return `Resuming — ${bytes}`;
      const kept = formatYouTubeBytes({ bytesCompleted: run.preservedBytes, bytesTotal: null });
      return kept ? `Resuming from ${kept}` : "Resuming…";
    }
  }
}

/** Fraction `0..1` for the progress bar of a resumed attempt, or null when unknown. */
export function resumeFraction(progress: ResumeProgress): number | null {
  if (!progress || progress.bytesCompleted === null) return null;
  if (progress.bytesTotal === null || progress.bytesTotal <= 0) return null;
  return Math.max(0, Math.min(1, progress.bytesCompleted / progress.bytesTotal));
}

/**
 * Text for the polite live region. It changes only when a run's state or
 * reason changes, never on byte ticks, so screen readers are not flooded.
 */
export function resumeAnnouncement(runs: readonly YouTubeResumableRun[]): string {
  if (runs.length === 0) return "";
  return runs
    .map((run) => `${run.title}: ${resumableStatusText(run, null)}`)
    .join(" ");
}

/** Runs that need a decision first, then those resuming, newest change last within each. */
export function orderResumable(runs: readonly YouTubeResumableRun[]): YouTubeResumableRun[] {
  const rank = (run: YouTubeResumableRun): number =>
    run.state === "needs_user" ? 0 : run.state === "auto_pending" ? 1 : 2;
  return [...runs].sort((left, right) => rank(left) - rank(right) || left.updatedAt - right.updatedAt);
}

/** True when two lists describe the same thing to the user, so a refresh need not re-render. */
export function sameResumableRuns(
  left: readonly YouTubeResumableRun[],
  right: readonly YouTubeResumableRun[]
): boolean {
  if (left.length !== right.length) return false;
  return left.every((run, index) => {
    const other = right[index];
    return other !== undefined
      && run.runId === other.runId
      && run.state === other.state
      && run.reason === other.reason
      && run.preservedBytes === other.preservedBytes
      && run.updatedAt === other.updatedAt;
  });
}
