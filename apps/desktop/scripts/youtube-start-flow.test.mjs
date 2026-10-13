import assert from "node:assert/strict";
import test from "node:test";
import {
  BUSY_RETRY_DELAYS_MS,
  BusyRetryCancelled,
  MAX_SELECTED_PER_RUN,
  batchingNotice,
  createBatchSequencer,
  playlistTruncationNote,
  splitIntoRuns,
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

const ids = (count, prefix = "occ") => Array.from({ length: count }, (_, index) => `${prefix}-${index}`);

test("a complete scan has no truncation note", () => {
  assert.equal(playlistTruncationNote({ title: "Mix", truncated: false, totalCount: 40, items: ids(40) }), null);
  assert.equal(playlistTruncationNote({ title: "Mix", truncated: false, totalCount: null, items: ids(500) }), null);
});

test("a truncated playlist says how many are shown, with the total and the title when known", () => {
  assert.equal(
    playlistTruncationNote({ title: "Daily Mix", truncated: true, totalCount: 1234, items: ids(500) }),
    'Showing the first 500 of 1,234 videos in the playlist "Daily Mix".'
  );
  assert.equal(
    playlistTruncationNote({ title: "Daily Mix", truncated: true, totalCount: null, items: ids(500) }),
    'Showing the first 500 videos of the playlist "Daily Mix".'
  );
  assert.equal(
    playlistTruncationNote({ title: "  ", truncated: true, totalCount: 900, items: ids(500) }),
    "Showing the first 500 of 900 videos in this playlist."
  );
});

test("a reported total that is not larger than what is shown is not claimed", () => {
  assert.equal(
    playlistTruncationNote({ title: "", truncated: true, totalCount: 500, items: ids(500) }),
    "Showing the first 500 videos of this playlist."
  );
});

test("selections within the per-run cap are left alone", () => {
  assert.equal(MAX_SELECTED_PER_RUN, 100);
  const groups = [{ scanPlanId: "a", occurrenceIds: ids(100) }, { scanPlanId: "b", occurrenceIds: ids(3, "b") }];
  assert.deepEqual(splitIntoRuns(groups), groups);
  assert.deepEqual(splitIntoRuns([]), []);
});

test("a larger selection queues as batches of at most the cap, in order, without losing or repeating a video", () => {
  const all = ids(250);
  const runs = splitIntoRuns([{ scanPlanId: "plan", occurrenceIds: all }]);
  assert.deepEqual(runs.map((run) => run.occurrenceIds.length), [100, 100, 50]);
  assert.ok(runs.every((run) => run.scanPlanId === "plan"));
  assert.deepEqual(runs.flatMap((run) => run.occurrenceIds), all);
  assert.equal(all.length, 250, "the input is not mutated");
});

test("batches are made per plan so a run never mixes scan plans", () => {
  const runs = splitIntoRuns([
    { scanPlanId: "a", occurrenceIds: ids(101, "a") },
    { scanPlanId: "b", occurrenceIds: ids(2, "b") }
  ]);
  assert.deepEqual(runs.map((run) => [run.scanPlanId, run.occurrenceIds.length]), [["a", 100], ["a", 1], ["b", 2]]);
});

test("the user is told only when a selection was split into batches", () => {
  assert.equal(batchingNotice(100, 1, 1), null);
  const notice = batchingNotice(250, 1, 3);
  assert.equal(notice?.title, "Downloading in batches");
  assert.match(notice?.description ?? "", /250 videos run as 3 batches of up to 100/);
});

// --- Batch sequencing -------------------------------------------------------------------------

/** A sequencer whose starts are scripted: `outcomes` is consumed one per start, in order. */
function harness(outcomes) {
  const starts = [];
  const notices = [];
  let next = 0;
  const sequencer = createBatchSequencer({
    start: async (group, context) => {
      starts.push({ plan: group.scanPlanId, ids: group.occurrenceIds.length, context });
      const outcome = outcomes[next] ?? { started: true, runId: `run-${starts.length}` };
      next += 1;
      return typeof outcome === "function" ? outcome() : outcome;
    },
    notify: (notice) => notices.push(notice)
  });
  return { sequencer, starts, notices };
}

const batches = (...sizes) => sizes.map((size, index) => ({ scanPlanId: `plan-${index + 1}`, occurrenceIds: ids(size, `b${index + 1}`) }));

test("the first batch starts at once and each later one only after the run before it ends", async () => {
  const { sequencer, starts, notices } = harness([]);
  await sequencer.begin(batches(100, 100, 50));
  assert.deepEqual(starts.map((start) => start.context), [{ batchNumber: 1, totalBatches: 3 }]);
  assert.equal(sequencer.pendingVideos(), 150);

  await sequencer.onRunTerminal("run-1");
  assert.deepEqual(starts.map((start) => start.context.batchNumber), [1, 2]);
  assert.equal(sequencer.pendingVideos(), 50);

  await sequencer.onRunTerminal("run-2");
  assert.deepEqual(starts.map((start) => start.context.batchNumber), [1, 2, 3]);
  assert.equal(sequencer.pendingVideos(), 0);

  await sequencer.onRunTerminal("run-3");
  assert.equal(starts.length, 3, "nothing is left to start");
  assert.deepEqual(notices, []);
});

test("a single group runs once and never waits for anything", async () => {
  const { sequencer, starts } = harness([]);
  await sequencer.begin(batches(10));
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 1);
});

test("cancel clears the queue so the ending run starts nothing", async () => {
  const { sequencer, starts, notices } = harness([]);
  await sequencer.begin(batches(100, 100, 50));
  sequencer.clear();
  assert.equal(sequencer.pendingVideos(), 0);
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 1);
  assert.deepEqual(notices, [], "a deliberate cancel is not reported as a failure");
});

test("a helper-reinstall clear empties the queue the same way, even after a batch has finished", async () => {
  const { sequencer, starts } = harness([]);
  await sequencer.begin(batches(100, 100, 50, 25));
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 2);
  sequencer.clear();
  await sequencer.onRunTerminal("run-2");
  assert.equal(starts.length, 2);
  assert.equal(sequencer.pendingVideos(), 0);
});

test("a mid-sequence start failure stops the sequence and reports how many videos were left", async () => {
  const { sequencer, starts, notices } = harness([
    undefined,
    { started: false, reason: "the plan expired" }
  ]);
  await sequencer.begin(batches(100, 100, 50, 25));
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 2);
  assert.equal(notices.length, 1);
  assert.equal(notices[0].title, "Remaining batches not started");
  assert.equal(notices[0].description, "Batch 2 of 4 couldn't start: the plan expired. 75 videos were not queued.");
  assert.equal(sequencer.pendingVideos(), 0);

  // The queue is gone: the failed batch's own run id never arrives, and a stray one starts nothing.
  await sequencer.onRunTerminal("run-2");
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 2);
});

test("a failure of the very first batch also reports the rest, but a lone batch has nothing to report", async () => {
  const first = harness([{ started: false, reason: null }]);
  await first.sequencer.begin(batches(100, 40));
  assert.equal(first.notices.length, 1);
  assert.equal(first.notices[0].description, "Batch 1 of 2 couldn't start. 40 videos were not queued.");

  const lone = harness([{ started: false, reason: "no folder" }]);
  await lone.sequencer.begin(batches(10));
  assert.deepEqual(lone.notices, []);
});

test("a start that throws is a failed start, not a stuck queue", async () => {
  const { sequencer, starts, notices } = harness([
    undefined,
    () => { throw new Error("ipc exploded"); }
  ]);
  await sequencer.begin(batches(100, 100, 30));
  await sequencer.onRunTerminal("run-1");
  assert.equal(notices[0].description, "Batch 2 of 3 couldn't start: ipc exploded. 30 videos were not queued.");
  assert.equal(sequencer.isStarting(), false);
  assert.equal(starts.length, 2);
});

test("a run advances the queue once: repeated, concurrent and foreign terminal notices start nothing extra", async () => {
  const { sequencer, starts } = harness([]);
  await sequencer.begin(batches(100, 100, 100));
  await Promise.all([sequencer.onRunTerminal("run-1"), sequencer.onRunTerminal("run-1"), sequencer.onRunTerminal("run-1")]);
  assert.equal(starts.length, 2, "three notices for one run start one batch");
  await sequencer.onRunTerminal("run-1");
  await sequencer.onRunTerminal("some-older-run");
  assert.equal(starts.length, 2);
  assert.equal(sequencer.pendingVideos(), 100);
});

test("a begin while a start is in flight is ignored and does not replace the queue", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const { sequencer, starts } = harness([() => gate]);
  const first = sequencer.begin(batches(100, 100));
  assert.equal(sequencer.isStarting(), true);
  await sequencer.begin(batches(5, 5, 5));
  assert.equal(starts.length, 1, "the second click did not start anything");
  release({ started: true, runId: "run-1" });
  await first;
  assert.equal(sequencer.pendingVideos(), 100, "the first click's queue is intact");
  assert.equal(sequencer.isStarting(), false);
});

test("a run that ends while its own start is still finishing is picked up by the next notice", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const { sequencer, starts } = harness([() => gate]);
  const first = sequencer.begin(batches(100, 100));
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 1, "too early: the start has not returned its run yet");
  release({ started: true, runId: "run-1" });
  await first;
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 2);
});

test("clearing while a start is in flight keeps the queue cleared and reports nothing", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const { sequencer, starts, notices } = harness([() => gate]);
  const first = sequencer.begin(batches(100, 100));
  sequencer.clear();
  release({ started: false, reason: "late failure" });
  await first;
  assert.deepEqual(notices, []);
  await sequencer.onRunTerminal("run-1");
  assert.equal(starts.length, 1);
});
