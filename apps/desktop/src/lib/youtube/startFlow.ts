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
  truncated: boolean;
  totalCount: number | null;
  items: readonly unknown[];
}): string | null {
  if (!plan.truncated) return null;
  const shown = COUNT_FORMAT.format(plan.items.length);
  if (plan.totalCount !== null && plan.totalCount > plan.items.length) {
    return `Showing the first ${shown} of ${COUNT_FORMAT.format(plan.totalCount)} videos in this playlist.`;
  }
  return `Showing the first ${shown} videos of this playlist.`;
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
