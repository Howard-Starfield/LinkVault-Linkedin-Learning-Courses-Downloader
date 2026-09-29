import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  nextPollDelayMs,
  ACTIVE_POLL_INTERVAL_MS,
  IDLE_POLL_CEILING_MS
} from "../src/lib/linkedin/poll-schedule.ts";

const NOW = 1_800_000_000_000; // ms
const sec = (offsetSeconds) => (NOW + offsetSeconds * 1000) / 1000;

const job = (overrides) => ({ status: "completed", paused: false, scheduled_at: null, ...overrides });

// --- The gate: stop polling when nothing can change on its own ---------------
assert.equal(nextPollDelayMs([], NOW), null, "An empty queue must stop polling");
assert.equal(
  nextPollDelayMs([job({}), job({ status: "failed" }), job({ status: "cancelled" })], NOW),
  null,
  "Completed/failed/cancelled jobs alone must stop polling"
);

// --- A running download keeps the fast cadence -------------------------------
assert.equal(
  nextPollDelayMs([job({ status: "active" })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "An active job must poll on the active cadence"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(3600) }), job({ status: "active" })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "An active job must win over a far-future scheduled job"
);

// --- Queued is NOT excludable: it is what starts scheduled downloads ---------
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: null })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "A queued job with no schedule is runnable now and must keep polling"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(-60) })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "A queued job whose schedule already passed is runnable now"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: undefined })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "A missing schedule must be treated as runnable, not as unknown"
);

// --- Sleep until the job is actually due -------------------------------------
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(1800) })], NOW),
  1_800_000,
  "A job due in 30 minutes must sleep exactly until it is due, not poll every 15s"
);
assert.equal(
  nextPollDelayMs(
    [
      job({ status: "queued", scheduled_at: sec(7200) }),
      job({ status: "queued", scheduled_at: sec(600) }),
      job({ status: "queued", scheduled_at: sec(3600) })
    ],
    NOW
  ),
  600_000,
  "The earliest upcoming job must win"
);
// The user-visible promise: a job queued for later costs one wake-up, not many.
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(600) })], NOW),
  600_000,
  "A job due in 10 minutes must cost exactly one wake-up"
);

// --- Floor, so a job due imminently cannot spin the loop ---------------------
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(0.2) })], NOW),
  1_000,
  "A job due in 200ms must still wait the 1s floor, never re-arm immediately"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(0) })], NOW),
  ACTIVE_POLL_INTERVAL_MS,
  "A job due exactly now is runnable and takes the active branch"
);

// --- Ceiling, the backstop against long schedules and clock/sleep drift ------
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(60 * 60 * 24 * 30) })], NOW),
  IDLE_POLL_CEILING_MS,
  "A job due in 30 days must be capped by the ceiling, not slept on directly"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", scheduled_at: sec(IDLE_POLL_CEILING_MS / 1000 + 60) })], NOW),
  IDLE_POLL_CEILING_MS,
  "A job just beyond the ceiling must clamp to exactly one ceiling interval"
);
assert.equal(
  nextPollDelayMs([job({ status: "queued", paused: true, scheduled_at: sec(600) })], NOW),
  IDLE_POLL_CEILING_MS,
  "A paused job never comes due on its own and must fall back to the ceiling"
);
assert.equal(
  nextPollDelayMs(
    [job({ status: "queued", paused: true }), job({ status: "queued", paused: true, scheduled_at: sec(60) })],
    NOW
  ),
  IDLE_POLL_CEILING_MS,
  "When every queued job is paused the ceiling applies"
);

// --- Mixed: one runnable job keeps the cadence even with others pending -----
assert.equal(
  nextPollDelayMs(
    [job({ status: "queued", scheduled_at: sec(7200) }), job({ status: "queued", scheduled_at: null })],
    NOW
  ),
  ACTIVE_POLL_INTERVAL_MS,
  "A runnable job must keep polling even when another job is scheduled for later"
);

// --- A paused job is inert: it must NOT buy a wake-up ------------------------
// `hasReadyQueuedJobs` and the backend's due-schedule query both require
// `paused = false`, so a paused job cannot start on its own. Waking for it would
// spend a poll to discover nothing. Unpausing is a user action, which re-arms.
assert.equal(
  nextPollDelayMs(
    [
      job({ status: "queued", paused: true, scheduled_at: sec(900) }),
      job({ status: "queued", paused: true, scheduled_at: sec(7200) })
    ],
    NOW,
  ),
  IDLE_POLL_CEILING_MS,
  "A paused job must not wake the poll for a schedule it cannot act on"
);

// ...but an unpaused job in the same batch still does, and wins over the paused one.
assert.equal(
  nextPollDelayMs(
    [
      job({ status: "queued", paused: true, scheduled_at: sec(900) }),
      job({ status: "queued", paused: false, scheduled_at: sec(1800) })
    ],
    NOW,
  ),
  1_800_000,
  "An unpaused scheduled job must still wake the poll even alongside paused jobs"
);

// --- The call site must actually use this, and must not poll unconditionally -
const appSource = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");

assert.ok(
  appSource.includes('from "./lib/linkedin/poll-schedule"'),
  "App.tsx must import the shared poll schedule helper"
);
assert.ok(
  appSource.includes("nextPollDelayMs(state.persisted_jobs, Date.now())"),
  "The LinkedIn poll must derive its next delay from the shared helper"
);
assert.equal(
  appSource.includes("window.setInterval(() => void checkDueSchedules(), 15_000)"),
  false,
  "The LinkedIn poll must no longer re-arm on an unconditional 15s interval"
);
assert.ok(
  appSource.includes("window.clearTimeout(timerId)"),
  "The poll effect must clear its timeout on cleanup so it cannot leak"
);
assert.ok(
  appSource.includes("if (disposed) return;"),
  "The poll effect must bail out when disposed so a stale timer cannot re-arm"
);

// The newspaper poll is a separate loop and must keep its own cadence.
assert.ok(
  appSource.includes("window.setInterval(() => void processNewspaperSchedules(), 15_000)"),
  "The newspaper poll is independent and must keep its existing interval"
);

console.log(
  `LinkedIn poll schedule passed: idle stops polling, active polls at ` +
    `${ACTIVE_POLL_INTERVAL_MS}ms, scheduled jobs sleep to their due time ` +
    `(floor 1000ms, ceiling ${IDLE_POLL_CEILING_MS}ms).`
);
