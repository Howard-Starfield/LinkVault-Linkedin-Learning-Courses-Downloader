import assert from "node:assert/strict";
import test from "node:test";
import {
  formatYouTubeBytes,
  youtubeItemStatusText,
  youtubeProgressDetail,
  youtubeProgressText,
  youtubeStageLabel
} from "../src/lib/youtube/itemStatus.ts";

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

test("byte counts use the total's decimal unit for both numbers", () => {
  const nb = "\u00A0";
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 18_300_000, bytesTotal: 43_600_000 }),
    ["18.3", "/", `43.6${nb}MB`].join(nb)
  );
  // A small completed amount is shown in the total's unit, not its own.
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 800_000, bytesTotal: 43_600_000 }),
    ["0.8", "/", `43.6${nb}MB`].join(nb)
  );
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 512, bytesTotal: 2_048 }),
    ["0.5", "/", `2.0${nb}KB`].join(nb)
  );
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 1_500_000_000, bytesTotal: 3_000_000_000 }),
    ["1.5", "/", `3.0${nb}GB`].join(nb)
  );
});

test("an unknown total shows only the completed bytes in their own unit", () => {
  const nb = "\u00A0";
  assert.equal(formatYouTubeBytes({ bytesCompleted: 18_300_000, bytesTotal: null }), `18.3${nb}MB`);
  assert.equal(formatYouTubeBytes({ bytesCompleted: 750, bytesTotal: null }), `750${nb}B`);
});

test("missing, zero or invalid byte progress shows nothing", () => {
  assert.equal(formatYouTubeBytes(null), null);
  assert.equal(formatYouTubeBytes({ bytesCompleted: null, bytesTotal: 100 }), null);
  assert.equal(formatYouTubeBytes({ bytesCompleted: 0, bytesTotal: 100 }), null);
  assert.equal(formatYouTubeBytes({ bytesCompleted: Number.NaN, bytesTotal: 100 }), null);
});

test("completed bytes never exceed the total", () => {
  const nb = "\u00A0";
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 5_000_000, bytesTotal: 4_000_000 }),
    ["4.0", "/", `4.0${nb}MB`].join(nb)
  );
});

test("a downloading row combines stage, percent and bytes", () => {
  const nb = "\u00A0";
  assert.equal(
    youtubeProgressText("Downloading video…", 42, { bytesCompleted: 18_300_000, bytesTotal: 43_600_000 }),
    `Downloading video… · 42% · 18.3${nb}/${nb}43.6${nb}MB`
  );
  assert.equal(
    youtubeProgressText("Downloading video…", 42.4, { bytesCompleted: 18_300_000, bytesTotal: null }),
    `Downloading video… · 42% · 18.3${nb}MB`
  );
});

test("the progress detail leaves the stage label out so only the label is announced", () => {
  const nb = "\u00A0";
  assert.equal(
    youtubeProgressDetail(42, { bytesCompleted: 18_300_000, bytesTotal: 43_600_000 }),
    `42% · 18.3${nb}/${nb}43.6${nb}MB`
  );
  assert.equal(youtubeProgressDetail(0, null), "");
  assert.equal(youtubeProgressDetail(100, null), "100%");
});

test("rows without bytes or percent keep the existing text", () => {
  assert.equal(youtubeProgressText("Fetching video…", 0, null), "Fetching video…");
  assert.equal(youtubeProgressText("Saved", 100, null), "Saved · 100%");
  assert.equal(
    youtubeProgressText("Downloading video…", 0, { bytesCompleted: null, bytesTotal: null }),
    "Downloading video…"
  );
  assert.equal(youtubeProgressText(null, 0, null), "");
});
