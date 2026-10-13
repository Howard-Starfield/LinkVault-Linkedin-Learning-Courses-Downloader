import assert from "node:assert/strict";
import test from "node:test";
import {
  formatYouTubeBytes,
  youtubeItemStatusText,
  youtubeProgressDetail,
  youtubeProgressText,
  youtubeStageLabel
} from "../src/lib/youtube/itemStatus.ts";
import { isYouTubeSimulatedRun } from "../src/lib/youtube/ipc.ts";

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

test("a value that rounds up to 1000 is promoted to the next unit", () => {
  const nb = "\u00A0";
  assert.equal(formatYouTubeBytes({ bytesCompleted: 999_950, bytesTotal: null }), `1.0${nb}MB`);
  assert.equal(formatYouTubeBytes({ bytesCompleted: 999_949, bytesTotal: null }), `999.9${nb}KB`);
  assert.equal(formatYouTubeBytes({ bytesCompleted: 999.6, bytesTotal: null }), `1.0${nb}KB`);
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 500_000, bytesTotal: 999_950 }),
    ["0.5", "/", `1.0${nb}MB`].join(nb)
  );
  assert.equal(formatYouTubeBytes({ bytesCompleted: 999_950_000, bytesTotal: null }), `1.0${nb}GB`);
});

test("a tiny completed amount against a huge total keeps its own unit instead of 0.0", () => {
  const nb = "\u00A0";
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 20_000, bytesTotal: 3_000_000_000 }),
    [`20.0${nb}KB`, "/", `3.0${nb}GB`].join(nb)
  );
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 1_024, bytesTotal: 3_000_000_000 }),
    [`1.0${nb}KB`, "/", `3.0${nb}GB`].join(nb)
  );
  // 0.1 GB is visible in the total's unit, so it stays there.
  assert.equal(
    formatYouTubeBytes({ bytesCompleted: 100_000_000, bytesTotal: 3_000_000_000 }),
    ["0.1", "/", `3.0${nb}GB`].join(nb)
  );
});

test("browser-only runs are recognised so their item counts are not shown as bytes", () => {
  assert.equal(isYouTubeSimulatedRun("preview-run-1760000000000"), true);
  assert.equal(isYouTubeSimulatedRun("youtube-ui-mock-1760000000000"), true);
  assert.equal(isYouTubeSimulatedRun("yt-5f2c1a"), false);
});
