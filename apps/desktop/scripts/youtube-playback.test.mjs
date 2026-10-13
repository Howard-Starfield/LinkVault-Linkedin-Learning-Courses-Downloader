import assert from "node:assert/strict";
import test from "node:test";
import {
  PROGRESS_SAVE_INTERVAL_MS,
  adjacentPlayableVideo,
  createProgressThrottle,
  createSerialRunner,
  describeVideoProgress,
  formatPlaybackClock,
  isPlaybackCompleted,
  playableVideos,
  resumePositionSeconds
} from "../src/lib/youtube/playback.ts";

function video(ordinal, overrides = {}) {
  return {
    occurrenceId: `occ-${ordinal}`,
    ordinal,
    title: `Video ${ordinal}`,
    videoId: `vid${ordinal}`,
    mediaUrl: `http://youtube-media.localhost/media/run/occ-${ordinal}`,
    subtitlesUrl: null,
    available: true,
    unavailableReason: null,
    positionSeconds: null,
    durationSeconds: null,
    completed: false,
    ...overrides
  };
}

const sample = (position, occurrenceId = "occ-1", duration = 100) => ({
  occurrenceId,
  positionSeconds: position,
  durationSeconds: duration
});

test("playing saves at most once per interval", () => {
  const throttle = createProgressThrottle();
  assert.equal(PROGRESS_SAVE_INTERVAL_MS, 5000);
  assert.equal(throttle.shouldSave(sample(1), 1_000, false), true);
  throttle.markSaved(sample(1), 1_000);
  assert.equal(throttle.shouldSave(sample(2), 2_000, false), false);
  assert.equal(throttle.shouldSave(sample(5), 5_999, false), false);
  assert.equal(throttle.shouldSave(sample(6), 6_000, false), true);
});

test("a sample that was never sent does not start the interval", () => {
  const throttle = createProgressThrottle();
  assert.equal(throttle.shouldSave(sample(1), 1_000, false), true);
  // markSaved was never called (nothing was sent), so the next tick may save.
  assert.equal(throttle.shouldSave(sample(2), 1_250, false), true);
});

test("forced saves bypass the interval but skip an unchanged position", () => {
  const throttle = createProgressThrottle();
  throttle.markSaved(sample(10), 1_000);
  assert.equal(throttle.shouldSave(sample(10.2), 1_100, true), false);
  assert.equal(throttle.shouldSave(sample(12), 1_100, true), true);
  assert.equal(throttle.shouldSave(sample(10.1, "occ-2"), 1_100, true), true);
});

test("invalid samples are never saved", () => {
  const throttle = createProgressThrottle();
  for (const bad of [
    sample(Number.NaN),
    sample(-1),
    sample(1, "occ-1", 0),
    sample(1, "occ-1", Number.POSITIVE_INFINITY),
    sample(1, "")
  ]) {
    assert.equal(throttle.shouldSave(bad, 1_000, true), false);
    assert.equal(throttle.shouldSave(bad, 1_000, false), false);
  }
});

test("resume restarts when unset, finished, or at the very end", () => {
  assert.equal(resumePositionSeconds(null), 0);
  assert.equal(resumePositionSeconds({ positionSeconds: 42, durationSeconds: 100, completed: false }), 42);
  assert.equal(resumePositionSeconds({ positionSeconds: 42, durationSeconds: 100, completed: true }), 0);
  assert.equal(resumePositionSeconds({ positionSeconds: 98, durationSeconds: 100, completed: false }), 0);
  assert.equal(resumePositionSeconds({ positionSeconds: 0, durationSeconds: 100, completed: false }), 0);
  assert.equal(resumePositionSeconds({ positionSeconds: Number.NaN, durationSeconds: 100, completed: false }), 0);
  assert.equal(resumePositionSeconds({ positionSeconds: 30, durationSeconds: 0, completed: false }), 30);
});

test("next and previous skip unavailable videos and stop at the ends", () => {
  const videos = [
    video(1),
    video(2, { available: false, mediaUrl: null, unavailableReason: "gone" }),
    video(3),
    video(4, { available: false, mediaUrl: null, unavailableReason: "gone" })
  ];
  assert.equal(adjacentPlayableVideo(videos, "occ-1", 1)?.occurrenceId, "occ-3");
  assert.equal(adjacentPlayableVideo(videos, "occ-3", -1)?.occurrenceId, "occ-1");
  assert.equal(adjacentPlayableVideo(videos, "occ-3", 1), null);
  assert.equal(adjacentPlayableVideo(videos, "occ-1", -1), null);
  assert.equal(adjacentPlayableVideo(videos, "missing", 1), null);
  assert.deepEqual(playableVideos(videos).map((item) => item.occurrenceId), ["occ-1", "occ-3"]);
});

test("clock and row status text", () => {
  assert.equal(formatPlaybackClock(0), "0:00");
  assert.equal(formatPlaybackClock(65.9), "1:05");
  assert.equal(formatPlaybackClock(3_725), "1:02:05");
  assert.equal(formatPlaybackClock(-1), "0:00");
  assert.equal(formatPlaybackClock(Number.NaN), "0:00");
  assert.equal(describeVideoProgress(video(1)), null);
  assert.equal(describeVideoProgress(video(1, { positionSeconds: 75, durationSeconds: 200 })), "Resume at 1:15");
  assert.equal(describeVideoProgress(video(1, { positionSeconds: 190, durationSeconds: 200, completed: true })), "Watched");
});

test("serial runner keeps saves in order and survives a failure", async () => {
  const runner = createSerialRunner();
  const order = [];
  const first = runner.run(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
    order.push("first");
    throw new Error("boom");
  });
  const second = runner.run(async () => {
    order.push("second");
    return 2;
  });
  await assert.rejects(first, /boom/);
  assert.equal(await second, 2);
  assert.deepEqual(order, ["first", "second"]);
});

test("completion matches the backend rule", () => {
  assert.equal(isPlaybackCompleted(50, 100), false);
  assert.equal(isPlaybackCompleted(95, 100), true);
  assert.equal(isPlaybackCompleted(9.8, 10), true);
  assert.equal(isPlaybackCompleted(4, 10), false);
  assert.equal(isPlaybackCompleted(3_000, 3_600), false);
  assert.equal(isPlaybackCompleted(3_590, 3_600), true);
  assert.equal(isPlaybackCompleted(Number.NaN, 100), false);
  assert.equal(isPlaybackCompleted(1, 0), false);
});
