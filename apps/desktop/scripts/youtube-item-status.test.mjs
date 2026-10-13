import assert from "node:assert/strict";
import test from "node:test";
import { youtubeItemStatusText, youtubeStageLabel } from "../src/lib/youtube/itemStatus.ts";

test("a pending item is Queued while the run is active and Not started once it ended", () => {
  assert.equal(youtubeItemStatusText("pending", "waiting", true, "active"), "Queued");
  assert.equal(youtubeItemStatusText("pending", "waiting", true, "terminal"), "Not started");
});

test("a video outside any run has no status", () => {
  assert.equal(youtubeItemStatusText("pending", null, true, "idle"), null);
});

test("a running item names its stage from the reported phase", () => {
  const expected = {
    waiting: "Fetching video…",
    media: "Downloading video…",
    transcript: "Downloading transcript…",
    normalizing_transcript: "Processing transcript…",
    verifying: "Checking media…",
    merging: "Merging…",
    finalizing: "Saving…"
  };
  for (const [phase, label] of Object.entries(expected)) {
    assert.equal(youtubeStageLabel(phase), label, phase);
    assert.equal(youtubeItemStatusText("running", phase, true, "active"), label, phase);
  }
});

test("a running item without a phase falls back to the first stage", () => {
  assert.equal(youtubeItemStatusText("running", null, true, "active"), "Fetching video…");
});

test("finished and failed items keep their plain status", () => {
  assert.equal(youtubeItemStatusText("completed", "completed", true, "terminal"), "Saved");
  assert.equal(youtubeItemStatusText("completed_with_warnings", "warning", true, "terminal"), "Saved with warnings");
  assert.equal(youtubeItemStatusText("failed", "failed", true, "terminal"), "Failed");
  assert.equal(youtubeItemStatusText("skipped_existing", "completed", true, "terminal"), "Already saved");
});

test("unavailable videos are labelled before anything else", () => {
  assert.equal(youtubeItemStatusText("skipped", null, false, "idle"), "Unavailable");
  assert.equal(youtubeItemStatusText("pending", null, false, "idle"), "Unconfirmed");
});
