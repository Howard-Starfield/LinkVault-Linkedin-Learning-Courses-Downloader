import type { YouTubeItemPhase, YouTubeItemState } from "./types";

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
const NBSP = " ";

function byteUnitIndex(bytes: number): number {
  let index = 0;
  let scaled = bytes;
  while (scaled >= 1000 && index < BYTE_UNITS.length - 1) {
    scaled /= 1000;
    index += 1;
  }
  return index;
}

function scaledBytes(bytes: number, unitIndex: number): string {
  const value = bytes / 1000 ** unitIndex;
  return unitIndex === 0 ? String(Math.round(value)) : value.toFixed(1);
}

/**
 * "18.3 / 43.6 MB" (decimal units, both numbers in the total's unit), or
 * "18.3 MB" while the total is unknown. Returns null when there is nothing
 * meaningful to show. Spaces are non-breaking so a narrow row wraps between
 * status segments, never inside the byte counts.
 */
export function formatYouTubeBytes(progress: YouTubeByteProgress | null): string | null {
  if (!progress) return null;
  const { bytesCompleted, bytesTotal } = progress;
  if (bytesCompleted === null || !Number.isFinite(bytesCompleted) || bytesCompleted <= 0) return null;
  const hasTotal = bytesTotal !== null && Number.isFinite(bytesTotal) && bytesTotal > 0;
  if (!hasTotal) {
    const unitIndex = byteUnitIndex(bytesCompleted);
    return `${scaledBytes(bytesCompleted, unitIndex)}${NBSP}${BYTE_UNITS[unitIndex]}`;
  }
  const unitIndex = byteUnitIndex(bytesTotal);
  const completed = Math.min(bytesCompleted, bytesTotal);
  return [
    scaledBytes(completed, unitIndex),
    "/",
    `${scaledBytes(bytesTotal, unitIndex)}${NBSP}${BYTE_UNITS[unitIndex]}`
  ].join(NBSP);
}

/** Stage text plus percent and, when known, bytes: "Downloading video… · 42% · 18.3 / 43.6 MB". */
export function youtubeProgressText(
  status: string | null,
  percent: number,
  bytes: YouTubeByteProgress | null
): string {
  const parts = [status ?? ""];
  if (percent > 0) parts.push(`${Math.round(percent)}%`);
  const byteText = formatYouTubeBytes(bytes);
  if (byteText) parts.push(byteText);
  return parts.filter((part) => part !== "").join(" · ");
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
