import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a running LinkedVault preview.");
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
try {
  for (const scenario of ["idle", "pending-later", "paused"]) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    await page.route("**/*", (route) => {
      const url = new URL(route.request().url());
      return url.origin === new URL(previewUrl).origin || url.protocol === "data:"
        ? route.continue() : route.abort();
    });
    await page.addInitScript(({ scenario }) => {
      const now = Math.floor(Date.now() / 1000);
      const jobs = scenario === "idle" ? [] : [{
        id: "fixture-existing", course_slug: "fixture-existing",
        source_url: "https://www.linkedin.com/learning/fixture-existing", status: "queued",
        title: "Fixture existing", paused: scenario === "paused",
        scheduled_at: now + (scenario === "paused" ? 60 : 1800), selected_quality: "720",
        output_dir: "C:/FixturePreview", created_at: now, updated_at: now
      }];
      sessionStorage.setItem("linkvault.preview.saved-token", "true");
      sessionStorage.setItem("linkvault.preview.preferences", JSON.stringify({
        outputDir: "C:/FixturePreview", selectedQuality: "720", delaySeconds: 0,
        videoWaitMinSeconds: 20, videoWaitMaxSeconds: 40, browserSource: "Chrome",
        downloadVideos: true, downloadExercises: true, downloadSubtitles: true, downloadQuizzes: true
      }));
      sessionStorage.setItem("linkvault.preview.jobs", JSON.stringify(jobs));
      sessionStorage.setItem("linkvault.preview.events", "[]");
      const originalSet = window.setTimeout.bind(window);
      const originalClear = window.clearTimeout.bind(window);
      window.__pollTimers = new Map();
      window.setTimeout = (callback, delay, ...args) => {
        const id = originalSet(callback, delay, ...args);
        if (String(callback).includes("tick")) window.__pollTimers.set(id, delay);
        return id;
      };
      window.clearTimeout = (id) => {
        window.__pollTimers.delete(id);
        return originalClear(id);
      };
    }, { scenario });
    await page.goto(previewUrl);
    await page.getByRole("textbox", { name: "Course URLs", exact: true }).waitFor();
    await page.waitForTimeout(300);
    const before = await page.evaluate(() => [...window.__pollTimers.values()]);
    assert.equal(before.length, scenario === "idle" ? 0 : 1, "Fixture must start with the expected timer count");
    if (scenario === "pending-later") assert.ok(before[0] > 1_700_000, "Fixture must have a 30-minute timer");
    if (scenario === "paused") assert.equal(before[0], 3_600_000, "Paused fixture must start on the backstop");
    if (scenario === "paused") {
      await page.getByRole("button", { name: "Resume Fixture existing", exact: true }).click();
    } else {
      await page.getByRole("textbox", { name: "Course URLs", exact: true }).fill("https://www.linkedin.com/learning/fixture-new-schedule");
      await page.getByRole("button", { name: "Schedule", exact: true }).click();
      await page.getByRole("spinbutton", { name: "Start within hours", exact: true }).fill("0");
      await page.getByRole("spinbutton", { name: "Start within additional minutes", exact: true }).fill("1");
      await page.getByRole("button", { name: "Review schedule", exact: true }).click();
      await page.getByRole("button", { name: "Confirm schedule", exact: true }).click();
    }
    await page.waitForTimeout(400);
    const timers = await page.evaluate(() => [...window.__pollTimers.values()]);
    assert.equal(timers.length, 1, "Rearming must replace the existing timer rather than duplicate it");
    assert.ok(timers[0] >= 1000 && timers[0] <= 60_000, `${scenario}: the one-minute schedule must replace the long timer, got ${timers[0]}ms`);
    // Ordinary input renders must keep the schedule timer rather than restart polling.
    const timerIds = await page.evaluate(() => [...window.__pollTimers.keys()]);
    await page.getByRole("textbox", { name: "Course URLs", exact: true }).fill("Fixture typing");
    await page.waitForTimeout(100);
    assert.deepEqual(await page.evaluate(() => [...window.__pollTimers.keys()]), timerIds, "Typing must retain the timer");
    console.log(`${scenario}: timer replaced with ${Math.round(timers[0])}ms; ordinary renders retain it.`);
    await page.close();
  }
} finally {
  await browser.close();
}
console.log("Mounted LinkedIn schedule checks passed with synthetic preview jobs; no native download was performed.");
