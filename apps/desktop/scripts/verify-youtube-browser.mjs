import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a running LinkVault preview.");

const browser = await chromium.launch({
  channel: process.env.PLAYWRIGHT_CHANNEL || "chrome",
  headless: true
});

try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
  page.setDefaultTimeout(10_000);
  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    const location = message.location();
    if (location.url && new URL(location.url).pathname === "/favicon.ico") return;
    consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));

  await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Open YouTube archive" }).click();
  const view = page.locator(".youtube-view");
  await view.waitFor();
  await page.getByRole("heading", { name: "YouTube archive" }).waitFor();

  const guidance = await page.locator(".youtube-guidance").innerText();
  assert.match(guidance, /public content you own or are authorized to save/i);
  assert.match(guidance, /private.*member-only.*paid.*age-gated/is);
  assert.equal(await page.locator('[aria-live="polite"]').count() > 0, true, "Live run announcer is missing");
  assert.equal(await page.getByLabel("Public YouTube URL").count(), 1);
  assert.equal(await page.getByLabel("YouTube output directory").count(), 1);

  const assertNoHorizontalLoss = async (label) => {
    const metrics = await page.evaluate(() => {
      const root = document.querySelector(".youtube-view");
      if (!root) return null;
      const controls = [...root.querySelectorAll("button, input, select")]
        .map((node) => node.getBoundingClientRect())
        .filter((rect) => rect.width > 0 && rect.height > 0);
      return {
        documentOverflow: document.documentElement.scrollWidth - window.innerWidth,
        rootOverflow: root.scrollWidth - root.clientWidth,
        controlOutside: controls.filter((rect) => rect.left < -1 || rect.right > window.innerWidth + 1).length
      };
    });
    assert.ok(metrics, `${label}: YouTube view is not mounted`);
    assert.ok(metrics.documentOverflow <= 2, `${label}: document has horizontal overflow (${metrics.documentOverflow}px)`);
    assert.ok(metrics.rootOverflow <= 2, `${label}: YouTube view has horizontal overflow (${metrics.rootOverflow}px)`);
    assert.equal(metrics.controlOutside, 0, `${label}: a visible control is outside the viewport`);
  };

  for (const profile of [
    { label: "narrow", width: 520, height: 900 },
    { label: "compact", width: 900, height: 900 },
    { label: "wide", width: 1440, height: 960 }
  ]) {
    await page.setViewportSize({ width: profile.width, height: profile.height });
    await assertNoHorizontalLoss(profile.label);
  }

  const acknowledgement = page.locator(".youtube-acknowledgement input");
  await acknowledgement.check();
  await page.getByLabel("Public YouTube URL").fill("https://www.youtube.com/playlist?list=PLlinkvault-preview");
  await page.getByLabel("YouTube output directory").fill("C:\\LinkVault\\YouTube-preview");
  await page.getByRole("button", { name: "Scan", exact: true }).click();
  await page.getByRole("list", { name: "Scanned YouTube occurrences" }).waitFor();

  const occurrenceChecks = page.locator(".youtube-reel-item input[type=checkbox]");
  assert.equal(await occurrenceChecks.count(), 4, "Preview playlist did not render four ordered occurrences");
  await occurrenceChecks.nth(0).check();
  assert.equal(await occurrenceChecks.nth(0).isChecked(), true, "Occurrence checkbox cannot be selected");
  await occurrenceChecks.nth(0).uncheck();
  await page.getByRole("button", { name: "Select all", exact: true }).click();
  assert.equal(
    await occurrenceChecks.evaluateAll((nodes) => nodes.every((node) => node instanceof HTMLInputElement && node.checked)),
    true,
    "Select all did not select every available occurrence"
  );

  await page.getByRole("button", { name: "Start selected (4)", exact: true }).click();
  await page.locator(".youtube-progress-block").waitFor();
  await page.locator(".youtube-run-panel .status-pill").filter({ hasText: "Running" }).waitFor();
  await page.getByRole("button", { name: "Cancel run", exact: true }).click();
  await page.locator(".youtube-run-panel .status-pill").filter({ hasText: "Cancelled" }).waitFor();
  await assertNoHorizontalLoss("post-cancel wide");

  assert.deepEqual(consoleErrors, [], `Browser emitted console/page errors: ${consoleErrors.join(" | ")}`);
  console.log("YouTube browser fixture passed at narrow, compact, and wide widths with scan, occurrence selection, progress, cancellation, and accessibility checks.");
} finally {
  await browser.close();
}
