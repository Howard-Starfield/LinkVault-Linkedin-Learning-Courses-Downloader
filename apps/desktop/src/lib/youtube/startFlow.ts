/** Rust code for "another discovery or run owns the single runtime slot right now". */
export const RUNTIME_BUSY_CODE = "RUNTIME_BUSY";

/**
 * Pauses between attempts while another lookup holds the discovery slot.
 * The pauses add up to about ten seconds; after that the busy error is shown.
 */
export const BUSY_RETRY_DELAYS_MS: readonly number[] = [200, 400, 800, 1200, 1600, 2000, 2000, 2000];

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
  /** Return false to stop retrying, e.g. once the click that started the work is superseded. */
  isCurrent?: () => boolean;
  /** Called once, before the first pause. */
  onWaiting?: () => void;
}

function defaultSleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}

/**
 * Runs `operation`, and while it fails with RUNTIME_BUSY waits and tries again
 * a bounded number of times.  Any other failure, and the last busy failure
 * once the pauses are used up, is thrown to the caller unchanged.
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
    try {
      return await operation();
    } catch (error: unknown) {
      const delay = delays[attempt];
      if (delay === undefined || !isRuntimeBusyError(error) || !isCurrent()) throw error;
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
