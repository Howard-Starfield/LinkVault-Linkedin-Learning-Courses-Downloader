import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Play, Square } from "lucide-react";
import { IconButton, Progress, Tooltip } from "../primitives";
import {
  cancelYouTubeDownload,
  continueYouTubeRun,
  formatYouTubeInvokeError,
  isTauriRuntime,
  listYouTubeResumableRuns,
  subscribeYouTubeResumeChanged,
  subscribeYouTubeRunChanged
} from "../../lib/youtube/ipc";
import {
  canContinueResumable,
  orderResumable,
  resumableStatusText,
  resumeAnnouncement,
  resumeFraction,
  resumeRefetchIntervalMs,
  sameResumableRuns,
  type ResumeProgress
} from "../../lib/youtube/itemStatus";
import type { YouTubeResumableRun } from "../../lib/youtube/types";

/**
 * Interrupted downloads: the one automatic resume in flight, runs paused
 * for the user with a Continue button, and resumed attempts with progress.
 * Renders nothing when there is nothing interrupted.
 */
export function YouTubeResumePanel() {
  const [runs, setRuns] = useState<YouTubeResumableRun[]>([]);
  const [progress, setProgress] = useState<ReadonlyMap<string, ResumeProgress>>(() => new Map());
  const [busyRunIds, setBusyRunIds] = useState<ReadonlySet<string>>(() => new Set());
  const busyRef = useRef<Set<string>>(new Set());
  const refreshGenerationRef = useRef(0);
  const mountedRef = useRef(true);

  const refresh = useCallback(async () => {
    const generation = ++refreshGenerationRef.current;
    try {
      const next = orderResumable(await listYouTubeResumableRuns());
      if (!mountedRef.current || generation !== refreshGenerationRef.current) return;
      setRuns((current) => (sameResumableRuns(current, next) ? current : next));
    } catch {
      // The list is a convenience view; a failed refresh keeps what is shown.
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    if (!isTauriRuntime()) return undefined;
    let disposed = false;
    const cleanups: Array<() => void> = [];
    void refresh();
    void subscribeYouTubeResumeChanged(() => void refresh()).then((cleanup) => {
      if (disposed) cleanup();
      else cleanups.push(cleanup);
    });
    void subscribeYouTubeRunChanged((event) => {
      if (disposed) return;
      setProgress((current) => {
        const known = current.get(event.runId);
        const bytesCompleted = event.progress.bytesCompleted;
        const bytesTotal = event.progress.bytesTotal;
        if (known?.bytesCompleted === bytesCompleted && known?.bytesTotal === bytesTotal) return current;
        const next = new Map(current);
        next.set(event.runId, { bytesCompleted, bytesTotal });
        return next;
      });
      // A run that ended or paused changes which list it belongs to.
      if (event.state !== "running" && event.state !== "pause_requested") void refresh();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else cleanups.push(cleanup);
    });
    return () => {
      disposed = true;
      mountedRef.current = false;
      for (const cleanup of cleanups) cleanup();
    };
  }, [refresh]);

  const announcement = useMemo(() => resumeAnnouncement(runs), [runs]);

  // Safety net for a missed event: refetch when the window regains focus, and
  // every 10 s while a resumed attempt is running (so a pause that ended it
  // always shows its Continue button).
  const refetchMs = resumeRefetchIntervalMs(runs);
  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    const timer = refetchMs > 0 ? window.setInterval(() => void refresh(), refetchMs) : null;
    return () => {
      window.removeEventListener("focus", onFocus);
      if (timer !== null) window.clearInterval(timer);
    };
  }, [refetchMs, refresh]);

  const markBusy = (runId: string, busy: boolean) => {
    if (busy) busyRef.current.add(runId);
    else busyRef.current.delete(runId);
    setBusyRunIds(new Set(busyRef.current));
  };

  async function handleContinue(run: YouTubeResumableRun): Promise<void> {
    // A double click lands on the ref before React re-renders the disabled button.
    if (busyRef.current.has(run.runId)) return;
    markBusy(run.runId, true);
    try {
      await continueYouTubeRun({ runId: run.runId, generation: run.generation });
    } catch (error) {
      toast.error("Could not continue the download", { description: formatYouTubeInvokeError(error) });
    } finally {
      markBusy(run.runId, false);
      void refresh();
    }
  }

  async function handleCancel(run: YouTubeResumableRun): Promise<void> {
    if (busyRef.current.has(run.runId)) return;
    markBusy(run.runId, true);
    try {
      await cancelYouTubeDownload({ runId: run.runId });
    } catch (error) {
      toast.error("Could not cancel the download", { description: formatYouTubeInvokeError(error) });
    } finally {
      markBusy(run.runId, false);
      void refresh();
    }
  }

  return (
    <>
      {/* One persistent live region, outside the list, so a status added later is still announced. */}
      <div className="youtube-live-announcer" role="status" aria-live="polite">{announcement}</div>
      {runs.length > 0 ? (
        <section className="youtube-resume-panel" aria-label="Interrupted downloads">
        <ul className="youtube-resume-list">
          {runs.map((run) => {
            const live = progress.get(run.runId) ?? null;
            const busy = busyRunIds.has(run.runId);
            const fraction = run.state === "resuming" ? resumeFraction(live) : null;
            const status = resumableStatusText(run, run.state === "resuming" ? live : null);
            return (
              <li key={run.runId} className="youtube-resume-row" data-state={run.state}>
                <div className="youtube-resume-copy">
                  <span className="youtube-resume-title" title={run.title}>
                    {run.title}
                    {run.videoCount > 1 ? ` + ${run.videoCount - 1} more` : ""}
                  </span>
                  <span className="youtube-resume-status">{status}</span>
                  {fraction !== null ? <Progress value={fraction * 100} className="youtube-resume-progress" /> : null}
                </div>
                <div className="youtube-resume-actions">
                  {canContinueResumable(run) ? (
                    <Tooltip label="Continue download">
                      <IconButton
                        type="button"
                        aria-label={`Continue ${run.title}`}
                        className="queue-pause-button youtube-resume-continue"
                        onClick={() => void handleContinue(run)}
                        loading={busy}
                        disabled={busy}
                      >
                        <Play aria-hidden="true" className="h-3.5 w-3.5" />
                      </IconButton>
                    </Tooltip>
                  ) : null}
                  <Tooltip label="Cancel and remove the partial download">
                    <IconButton
                      type="button"
                      aria-label={`Cancel ${run.title}`}
                      className="queue-remove-button youtube-resume-cancel"
                      onClick={() => void handleCancel(run)}
                      disabled={busy}
                    >
                      <Square aria-hidden="true" className="h-3.5 w-3.5" />
                    </IconButton>
                  </Tooltip>
                </div>
              </li>
            );
          })}
        </ul>
        </section>
      ) : null}
    </>
  );
}
