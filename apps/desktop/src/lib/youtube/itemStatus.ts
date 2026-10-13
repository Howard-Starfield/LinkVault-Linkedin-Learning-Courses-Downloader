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
