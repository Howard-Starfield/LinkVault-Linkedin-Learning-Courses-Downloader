import assert from "node:assert/strict";
import test from "node:test";
import {
  PROGRESS_SAVE_INTERVAL_MS,
  adjacentPlayableVideo,
  createEndedAdvance,
  createProgressSaver,
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
  throttle.markAttempted(sample(1), 1_000);
  throttle.markSaved(1_000);
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
  throttle.markAttempted(sample(10), 1_000);
  throttle.markSaved(1_000);
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

const sampleAt = (position, occurrenceId = "occ-1") => ({ occurrenceId, positionSeconds: position, durationSeconds: 100 });

test("close waits for a pending save before it resolves", async () => {
  const runner = createSerialRunner();
  const resolvers = [];
  const saver = createProgressSaver({
    runner,
    now: () => 1_000,
    save: () => new Promise((resolve) => resolvers.push(resolve))
  });
  const first = saver.request(sampleAt(10), true);
  let closed = false;
  const close = saver.request(sampleAt(10), true).then(() => {
    closed = true;
  });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(closed, false, "close must not resolve while a save is still in flight");
  while (!closed) {
    resolvers.splice(0).forEach((resolve) => resolve());
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  await Promise.all([first, close]);
});

test("an unchanged forced save still waits for the queue to drain", async () => {
  const runner = createSerialRunner();
  const saver = createProgressSaver({ runner, now: () => 1_000, save: async () => undefined });
  await saver.request(sampleAt(10), true);
  let release;
  const slow = runner.run(() => new Promise((resolve) => {
    release = resolve;
  }));
  let closed = false;
  // Position unchanged and already saved: nothing to send, yet it must wait.
  const close = saver.request(sampleAt(10), true).then(() => {
    closed = true;
  });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(closed, false);
  release();
  await slow;
  await close;
  assert.equal(closed, true);
});

test("a failed save stays dirty and is retried by the next forced save", async () => {
  const runner = createSerialRunner();
  const calls = [];
  const errors = [];
  let fail = true;
  const saver = createProgressSaver({
    runner,
    now: () => 1_000,
    onError: (error) => errors.push(error),
    save: async (sample) => {
      calls.push(sample.positionSeconds);
      if (fail) throw new Error("ipc down");
    }
  });
  await saver.request(sampleAt(10), false);
  assert.equal(errors.length, 1);
  fail = false;
  await saver.request(sampleAt(10), true);
  assert.deepEqual(calls, [10, 10]);
  // Now it succeeded, so an unchanged forced save is skipped.
  await saver.request(sampleAt(10), true);
  assert.deepEqual(calls, [10, 10]);
});

test("a failed periodic save is not retried before the interval", async () => {
  let now = 1_000;
  const calls = [];
  const saver = createProgressSaver({
    runner: createSerialRunner(),
    now: () => now,
    onError: () => undefined,
    save: async (sample) => {
      calls.push(sample.positionSeconds);
      throw new Error("down");
    }
  });
  await saver.request(sampleAt(1), false);
  now = 2_000;
  await saver.request(sampleAt(2), false);
  assert.deepEqual(calls, [1]);
  now = 6_100;
  await saver.request(sampleAt(7), false);
  assert.deepEqual(calls, [1, 7]);
});

test("a pause back at a committed value is not skipped while a different save is pending", async () => {
  const runner = createSerialRunner();
  const saved = [];
  const resolvers = [];
  const saver = createProgressSaver({
    runner,
    now: () => 1_000,
    save: (sample) => {
      saved.push(sample.positionSeconds);
      return new Promise((resolve) => resolvers.push(resolve));
    }
  });
  const flush = async () => {
    for (let i = 0; i < 5; i += 1) {
      resolvers.splice(0).forEach((resolve) => resolve());
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  };
  const first = saver.request(sampleAt(10), true);
  await flush();
  await first; // 10 is committed
  const second = saver.request(sampleAt(20), true); // 20 is now pending
  await new Promise((resolve) => setTimeout(resolve, 5));
  const third = saver.request(sampleAt(10), true); // seek back to 10 and pause
  await flush();
  await Promise.all([second, third]);
  assert.deepEqual(saved, [10, 20, 10], "the last write must be the pause at 10");
});

test("ended advance is invalidated by play, seeking, or a changed ended state", async () => {
  let advances = 0;
  const advance = createEndedAdvance({ onAdvance: () => { advances += 1; } });

  // Normal case: still ended once the save finishes.
  await advance.afterSave(Promise.resolve(), () => true);
  assert.equal(advances, 1);

  // The user restarts the same video before the save finishes.
  let finish;
  const pending = advance.afterSave(new Promise((resolve) => { finish = resolve; }), () => true);
  advance.invalidate(); // "play" or "seeking" fired
  finish();
  await pending;
  assert.equal(advances, 1);

  // No longer at the end (for example it was paused earlier in the video).
  await advance.afterSave(Promise.resolve(), () => false);
  assert.equal(advances, 1);

  // Unmount / source change invalidates too, and later ends work again.
  const stale = advance.afterSave(Promise.resolve(), () => true);
  advance.invalidate();
  await stale;
  assert.equal(advances, 1);
  await advance.afterSave(Promise.resolve(), () => true);
  assert.equal(advances, 2);
});
