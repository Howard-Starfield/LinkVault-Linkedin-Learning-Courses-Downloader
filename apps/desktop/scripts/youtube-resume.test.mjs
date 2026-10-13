import assert from "node:assert/strict";
import test from "node:test";
import {
  canContinueResumable,
  orderResumable,
  resumableStatusText,
  resumeAnnouncement,
  resumeFraction,
  resumeRefetchIntervalMs,
  sameResumableRuns
} from "../src/lib/youtube/itemStatus.ts";
import { isYouTubeRunSuspended } from "../src/lib/youtube/types.ts";

function run(overrides = {}) {
  return {
    runId: "run-1",
    state: "needs_user",
    reason: null,
    reasonCode: null,
    title: "Long talk",
    videoCount: 1,
    outputDir: "C:\\Videos",
    autoResumeUsed: true,
    preservedBytes: 0,
    updatedAt: 10,
    generation: 1,
    ...overrides
  };
}

test("an interrupted run says it is resuming automatically", () => {
  assert.equal(
    resumableStatusText(run({ state: "auto_pending" }), null),
    "Interrupted — resuming automatically…"
  );
});

test("a paused run names its reason", () => {
  assert.equal(
    resumableStatusText(run({ reason: "The disk filled up." }), null),
    "Paused — The disk filled up."
  );
  assert.match(resumableStatusText(run({ reason: "  " }), null), /^Paused — /);
});

test("a resumed attempt starts from the preserved bytes, then follows yt-dlp", () => {
  const resuming = run({ state: "resuming", preservedBytes: 120_000_000 });
  assert.equal(resumableStatusText(resuming, null), "Resuming from 120.0\u00A0MB");
  assert.equal(
    resumableStatusText(resuming, { bytesCompleted: 600_000_000, bytesTotal: 1_200_000_000 }),
    "Resuming — 0.6\u00A0/\u00A01.2\u00A0GB"
  );
  assert.equal(resumableStatusText(run({ state: "resuming" }), null), "Resuming…");
});

test("only runs waiting for a decision can be continued", () => {
  assert.equal(canContinueResumable(run({ state: "needs_user" })), true);
  assert.equal(canContinueResumable(run({ state: "auto_pending" })), true);
  assert.equal(canContinueResumable(run({ state: "resuming" })), false);
});

test("the resume fraction needs both bytes and a total", () => {
  assert.equal(resumeFraction(null), null);
  assert.equal(resumeFraction({ bytesCompleted: 5, bytesTotal: null }), null);
  assert.equal(resumeFraction({ bytesCompleted: 50, bytesTotal: 200 }), 0.25);
  assert.equal(resumeFraction({ bytesCompleted: 500, bytesTotal: 200 }), 1);
});

test("the announcement ignores byte ticks and lists each run once", () => {
  const runs = [run({ title: "A", reason: "No disk." }), run({ runId: "b", title: "B", state: "auto_pending" })];
  assert.equal(
    resumeAnnouncement(runs),
    "A: Paused — No disk. B: Interrupted — resuming automatically…"
  );
  assert.equal(resumeAnnouncement([]), "");
});

test("runs waiting for the user come first", () => {
  const ordered = orderResumable([
    run({ runId: "a", state: "resuming", updatedAt: 1 }),
    run({ runId: "b", state: "needs_user", updatedAt: 5 }),
    run({ runId: "c", state: "auto_pending", updatedAt: 2 })
  ]);
  assert.deepEqual(ordered.map((entry) => entry.runId), ["b", "c", "a"]);
});

test("equal lists are recognised so a refresh does not re-render", () => {
  assert.equal(sameResumableRuns([run()], [run()]), true);
  assert.equal(sameResumableRuns([run()], [run({ reason: "x" })]), false);
  assert.equal(sameResumableRuns([run()], []), false);
});

test("a paused snapshot with an error is suspended, a cooperative pause is not", () => {
  assert.equal(isYouTubeRunSuspended({ state: "paused", error: { code: "X", message: "y" } }), true);
  assert.equal(isYouTubeRunSuspended({ state: "paused", error: null }), false);
  assert.equal(isYouTubeRunSuspended({ state: "running", error: null }), false);
  assert.equal(isYouTubeRunSuspended(null), false);
});

test("a new pause is a different list even when nothing else changed", () => {
  assert.equal(sameResumableRuns([run({ generation: 1 })], [run({ generation: 2 })]), false);
});

test("a safety refetch runs only while a resumed attempt is running", () => {
  assert.equal(resumeRefetchIntervalMs([run({ state: "resuming" })]), 10_000);
  assert.equal(resumeRefetchIntervalMs([run({ state: "needs_user" }), run({ state: "auto_pending" })]), 0);
  assert.equal(resumeRefetchIntervalMs([]), 0);
});
