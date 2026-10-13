/** Rust code for "another discovery or run owns the single runtime slot right now". */
export const RUNTIME_BUSY_CODE = "RUNTIME_BUSY";

/**
 * Pauses between attempts while another lookup holds the discovery slot.
 * The pauses add up to about ten seconds; after that the busy error is shown.
 */
export const BUSY_RETRY_DELAYS_MS: readonly number[] = [200, 400, 800, 1200, 1600, 2000, 2000, 2000];

/** Most occurrences one run accepts; the backend refuses more (MAX_SELECTED_ITEMS). */
export const MAX_SELECTED_PER_RUN = 100;

const COUNT_FORMAT = new Intl.NumberFormat("en-US");

/**
 * The note for a scan that kept only the first entries of a larger playlist, or null when the
 * scan is complete.  Uses the playlist's total when YouTube reported one.
 */
export function playlistTruncationNote(plan: {
  title: string;
  truncated: boolean;
  totalCount: number | null;
  items: readonly unknown[];
}): string | null {
  if (!plan.truncated) return null;
  const shown = COUNT_FORMAT.format(plan.items.length);
  const title = plan.title.trim();
  const place = title ? `the playlist "${title}"` : "this playlist";
  if (plan.totalCount !== null && plan.totalCount > plan.items.length) {
    return `Showing the first ${shown} of ${COUNT_FORMAT.format(plan.totalCount)} videos in ${place}.`;
  }
  return `Showing the first ${shown} videos of ${place}.`;
}

/**
 * Splits download groups so no run exceeds the per-run selection cap; the groups then run one
 * after another through the existing queue.  Groups within the cap come back unchanged.
 */
export function splitIntoRuns<Group extends { occurrenceIds: readonly string[] }>(
  groups: readonly Group[],
  limit: number = MAX_SELECTED_PER_RUN
): Group[] {
  const runs: Group[] = [];
  for (const group of groups) {
    for (let start = 0; start < group.occurrenceIds.length; start += limit) {
      runs.push({ ...group, occurrenceIds: group.occurrenceIds.slice(start, start + limit) });
    }
  }
  return runs;
}

export type BatchStartResult =
  | { started: true; runId: string }
  /** `reason` is a short human reason, or null when the start already explained itself. */
  | { started: false; reason: string | null };

export interface BatchContext {
  /** 1-based number of the batch being started. */
  batchNumber: number;
  totalBatches: number;
}

export interface BatchSequencerPorts<Group> {
  start(group: Group, context: BatchContext): Promise<BatchStartResult>;
  /** Tells the user that the remaining batches were not started. */
  notify(notice: { title: string; description: string }): void;
}

export interface BatchSequencer<Group> {
  /** Starts the first batch and queues the rest.  Ignored while a start is in flight. */
  begin(groups: readonly Group[]): Promise<void>;
  /** The run `runId` reached a terminal state; starts the next batch if this sequencer awaits that run. */
  onRunTerminal(runId: string): Promise<void>;
  /** Drops every batch not yet started (cancel, helper reinstall, a rescan). */
  clear(): void;
  isStarting(): boolean;
  pendingVideos(): number;
}

/**
 * Runs download batches one after another.  Each batch's start is injected, so the sequencing is
 * independent of the view: the next batch starts only after the run it awaits ends, a failed start
 * stops the sequence and says how many videos were left, `clear` empties the queue at any time, and
 * a run can never advance the queue twice.
 */
export function createBatchSequencer<Group extends { occurrenceIds: readonly string[] }>(
  ports: BatchSequencerPorts<Group>
): BatchSequencer<Group> {
  let pending: Group[] = [];
  let totalBatches = 0;
  let batchNumber = 0;
  let starting = false;
  /** The run whose end starts the next batch. */
  let awaitedRunId: string | null = null;
  /** Bumped by `clear` and `begin` so an in-flight start cannot revive a cleared queue. */
  let epoch = 0;

  const pendingVideos = () => pending.reduce((total, group) => total + group.occurrenceIds.length, 0);

  async function launch(group: Group): Promise<void> {
    starting = true;
    awaitedRunId = null;
    batchNumber += 1;
    const launchEpoch = epoch;
    const number = batchNumber;
    let result: BatchStartResult;
    try {
      result = await ports.start(group, { batchNumber: number, totalBatches });
    } catch (error: unknown) {
      result = { started: false, reason: error instanceof Error ? error.message : null };
    } finally {
      starting = false;
    }
    if (launchEpoch !== epoch) return;
    if (result.started) {
      awaitedRunId = result.runId;
      return;
    }
    const remaining = pendingVideos();
    pending = [];
    if (remaining > 0) {
      const why = result.reason ? `: ${result.reason}` : "";
      ports.notify({
        title: "Remaining batches not started",
        description: `Batch ${number} of ${totalBatches} couldn't start${why}. ${COUNT_FORMAT.format(remaining)} videos were not queued.`
      });
    }
  }

  return {
    async begin(groups) {
      if (starting) return;
      const [first, ...rest] = groups;
      if (!first) return;
      epoch += 1;
      pending = rest;
      totalBatches = groups.length;
      batchNumber = 0;
      await launch(first);
    },
    async onRunTerminal(runId) {
      if (starting || awaitedRunId === null || awaitedRunId !== runId) return;
      awaitedRunId = null;
      const next = pending.shift();
      if (next === undefined) return;
      await launch(next);
    },
    clear() {
      epoch += 1;
      pending = [];
      awaitedRunId = null;
    },
    isStarting: () => starting,
    pendingVideos
  };
}

/** Tells the user that a large selection runs as several batches, or null when it is a single run per group. */
export function batchingNotice(
  videoCount: number,
  groupCount: number,
  runCount: number
): { title: string; description: string } | null {
  if (runCount <= groupCount) return null;
  return {
    title: "Downloading in batches",
    description: `${COUNT_FORMAT.format(videoCount)} videos run as ${runCount} batches of up to ${MAX_SELECTED_PER_RUN}, one after another.`
  };
}

/** Shown while a Download waits for the lookup that currently owns the slot. */
export const WAITING_FOR_LOOKUP_MESSAGE = "Waiting for the current lookup…";

const STILL_BUSY_MESSAGE = "Another lookup is still running. Try again in a moment.";

/**
 * Reads the code of a failed invoke: Rust sends `{ code, message }`, and some paths flatten it to
 * "CODE: message". (Kept free of imports so the node tests can load this file directly.)
 */
function errorCodeOf(error: unknown): string | null {
  if (typeof error === "string") {
    return /^([A-Z][A-Z0-9_]+):/.exec(error.trim())?.[1] ?? null;
  }
  if (typeof error === "object" && error !== null && "code" in error) {
    const { code } = error;
    return typeof code === "string" ? code : null;
  }
  return null;
}

export function isRuntimeBusyError(error: unknown): boolean {
  return errorCodeOf(error) === RUNTIME_BUSY_CODE;
}

export interface BusyRetryOptions {
  /** Pauses between attempts; one retry per entry. */
  delaysMs?: readonly number[];
  sleep?: (milliseconds: number) => Promise<void>;
  /**
   * Return false once the work is no longer wanted (the view unmounted, or the click that started
   * it was superseded).  It is checked before every attempt, including the first and the one that
   * follows each pause, so a cancelled wait never starts the operation.
   */
  isCurrent?: () => boolean;
  /** Called once, before the first pause. */
  onWaiting?: () => void;
}

/** Thrown by {@link retryWhileBusy} when `isCurrent` turned false; the operation was not (re)run. */
export class BusyRetryCancelled extends Error {
  constructor() {
    super("The wait for the current lookup was cancelled.");
    this.name = "BusyRetryCancelled";
  }
}

function defaultSleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}

/**
 * Runs `operation`, and while it fails with RUNTIME_BUSY waits and tries again
 * a bounded number of times.  Any other failure, and the last busy failure
 * once the pauses are used up, is thrown to the caller unchanged.  If
 * `isCurrent` turns false the wait ends with {@link BusyRetryCancelled} and the
 * operation is not run again.
 */
export async function retryWhileBusy<T>(
  operation: () => Promise<T>,
  options: BusyRetryOptions = {}
): Promise<T> {
  const delays = options.delaysMs ?? BUSY_RETRY_DELAYS_MS;
  const sleep = options.sleep ?? defaultSleep;
  const isCurrent = options.isCurrent ?? (() => true);
  let announced = false;
  for (let attempt = 0; ; attempt += 1) {
    if (!isCurrent()) throw new BusyRetryCancelled();
    try {
      return await operation();
    } catch (error: unknown) {
      const delay = delays[attempt];
      if (delay === undefined || !isRuntimeBusyError(error)) throw error;
      if (!isCurrent()) throw new BusyRetryCancelled();
      if (!announced) {
        announced = true;
        options.onWaiting?.();
      }
      await sleep(delay);
    }
  }
}

/** Explains a failure that survived the busy retries; null leaves the raw message to the caller. */
export function describeBusyFailure(error: unknown): string | null {
  return isRuntimeBusyError(error) ? STILL_BUSY_MESSAGE : null;
}

/**
 * How a transcript inspection requested by a Download click ended.  Every
 * outcome other than `ready` has already told the user why, or must be told
 * by the caller (`superseded`), so a click is never dropped silently.
 */
export type InspectionOutcome<Response> =
  | { kind: "ready"; response: Response }
  /** A newer scan or request replaced this one while it was in flight. */
  | { kind: "superseded" }
  /** The inspection failed and an error toast was shown. */
  | { kind: "failed" };

/** Message for the one outcome the inspection itself does not announce. */
export function supersededClickMessage(): { title: string; description: string } {
  return {
    title: "The video list changed",
    description: "Click Download again to start from the updated list."
  };
}

/**
 * A row's Download button is unavailable while any start is in flight: its own
 * row shows the loading state, the others are disabled so a click can never be
 * swallowed.
 */
export function isRowDownloadDisabled(input: {
  available: boolean;
  activeRun: boolean;
  helperReady: boolean;
  startInFlight: boolean;
}): boolean {
  return !input.available || input.activeRun || !input.helperReady || input.startInFlight;
}
