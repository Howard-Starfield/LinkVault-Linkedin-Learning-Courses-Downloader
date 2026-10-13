import assert from "node:assert/strict";
import test from "node:test";
import {
  BUSY_RETRY_DELAYS_MS,
  BusyRetryCancelled,
  describeBusyFailure,
  isRowDownloadDisabled,
  isRuntimeBusyError,
  retryWhileBusy,
  supersededClickMessage
} from "../src/lib/youtube/startFlow.ts";

const busy = () => ({ code: "RUNTIME_BUSY", message: "another discovery or run is active" });

function recorder() {
  const slept = [];
  return { slept, sleep: async (milliseconds) => { slept.push(milliseconds); } };
}

test("RUNTIME_BUSY is recognised in object and string form, and nothing else is", () => {
  assert.equal(isRuntimeBusyError(busy()), true);
  assert.equal(isRuntimeBusyError("RUNTIME_BUSY: another discovery or run is active"), true);
  assert.equal(isRuntimeBusyError({ code: "HELPER_FAILED", message: "RUNTIME_BUSY" }), false);
  assert.equal(isRuntimeBusyError(new Error("RUNTIME_BUSY")), false);
  assert.equal(isRuntimeBusyError(null), false);
});

test("the busy retries total about ten seconds", () => {
  const total = BUSY_RETRY_DELAYS_MS.reduce((sum, delay) => sum + delay, 0);
  assert.ok(total >= 9_000 && total <= 11_000, `total ${total}`);
  for (let index = 1; index < BUSY_RETRY_DELAYS_MS.length; index += 1) {
    assert.ok(BUSY_RETRY_DELAYS_MS[index] >= BUSY_RETRY_DELAYS_MS[index - 1], "delays back off");
  }
});

test("an operation that is not busy runs once and never waits", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  let waiting = 0;
  const value = await retryWhileBusy(async () => { calls += 1; return "done"; }, { sleep, onWaiting: () => { waiting += 1; } });
  assert.equal(value, "done");
  assert.equal(calls, 1);
  assert.equal(waiting, 0);
  assert.deepEqual(slept, []);
});

test("a busy slot is waited for and the retry then succeeds", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  let waiting = 0;
  const value = await retryWhileBusy(
    async () => {
      calls += 1;
      if (calls < 3) throw busy();
      return "started";
    },
    { sleep, onWaiting: () => { waiting += 1; } }
  );
  assert.equal(value, "started");
  assert.equal(calls, 3);
  assert.equal(waiting, 1, "the wait is announced once, not per attempt");
  assert.deepEqual(slept, BUSY_RETRY_DELAYS_MS.slice(0, 2));
});

test("a slot that stays busy fails with the original busy error after the bounded wait", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  const failure = busy();
  await assert.rejects(
    retryWhileBusy(async () => { calls += 1; throw failure; }, { sleep }),
    (error) => error === failure
  );
  assert.equal(calls, BUSY_RETRY_DELAYS_MS.length + 1);
  assert.deepEqual(slept, [...BUSY_RETRY_DELAYS_MS]);
});

test("any other failure is thrown at once without waiting", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  const failure = { code: "SCAN_PLAN_STALE", message: "scan plan no longer matches" };
  await assert.rejects(
    retryWhileBusy(async () => { calls += 1; throw failure; }, { sleep }),
    (error) => error === failure
  );
  assert.equal(calls, 1);
  assert.deepEqual(slept, []);
});

test("a click that was superseded stops waiting", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  let current = true;
  await assert.rejects(
    retryWhileBusy(
      async () => {
        calls += 1;
        current = false;
        throw busy();
      },
      { sleep, isCurrent: () => current }
    ),
    (error) => error instanceof BusyRetryCancelled
  );
  assert.equal(calls, 1);
  assert.deepEqual(slept, []);
});

test("work that stops being wanted during a pause is not run again", async () => {
  let calls = 0;
  let current = true;
  let waiting = 0;
  await assert.rejects(
    retryWhileBusy(
      async () => {
        calls += 1;
        throw busy();
      },
      {
        // The view unmounts (or the scan is replaced) while the pause is running.
        sleep: async () => { current = false; },
        isCurrent: () => current,
        onWaiting: () => { waiting += 1; }
      }
    ),
    (error) => error instanceof BusyRetryCancelled
  );
  assert.equal(calls, 1, "no attempt follows the cancelled pause");
  assert.equal(waiting, 1);
});

test("work that is already unwanted is never started", async () => {
  let calls = 0;
  await assert.rejects(
    retryWhileBusy(async () => { calls += 1; return "started"; }, { isCurrent: () => false }),
    (error) => error instanceof BusyRetryCancelled
  );
  assert.equal(calls, 0);
});

test("cancellation does not mask other failures while the work is still wanted", async () => {
  const failure = { code: "SCAN_PLAN_STALE", message: "stale" };
  await assert.rejects(
    retryWhileBusy(async () => { throw failure; }, { isCurrent: () => true }),
    (error) => error === failure
  );
});

test("custom pauses bound the retries", async () => {
  const { slept, sleep } = recorder();
  let calls = 0;
  await assert.rejects(retryWhileBusy(async () => { calls += 1; throw busy(); }, { sleep, delaysMs: [5] }));
  assert.equal(calls, 2);
  assert.deepEqual(slept, [5]);
});

test("a busy failure that survives the wait gets a plain explanation, others keep their own text", () => {
  assert.match(describeBusyFailure(busy()) ?? "", /still running/);
  assert.equal(describeBusyFailure({ code: "HELPER_FAILED", message: "x" }), null);
});

test("a superseded click always has a user-facing message", () => {
  const notice = supersededClickMessage();
  assert.ok(notice.title.length > 0);
  assert.match(notice.description, /Download again/);
});

test("a row's Download is disabled while any start is in flight, and otherwise on the usual guards", () => {
  const base = { available: true, activeRun: false, helperReady: true, startInFlight: false };
  assert.equal(isRowDownloadDisabled(base), false);
  assert.equal(isRowDownloadDisabled({ ...base, startInFlight: true }), true);
  assert.equal(isRowDownloadDisabled({ ...base, available: false }), true);
  assert.equal(isRowDownloadDisabled({ ...base, activeRun: true }), true);
  assert.equal(isRowDownloadDisabled({ ...base, helperReady: false }), true);
});
