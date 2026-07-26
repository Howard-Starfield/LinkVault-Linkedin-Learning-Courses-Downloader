import assert from "node:assert/strict";
import { chromium } from "playwright";

const url = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(url, "Set LINKVAULT_PREVIEW_URL to a running local LinkVault preview.");
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1720, height: 960 } });
await page.goto(url);
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));
const linkedInDownloadButton = page.getByRole("button", { name: "Start Download", exact: true });
const linkedInButtonBox = await linkedInDownloadButton.boundingBox();
const linkedInButtonClass = await linkedInDownloadButton.getAttribute("class");
const linkedInScheduleButton = page.getByRole("button", { name: "Schedule", exact: true });
const linkedInScheduleButtonBox = await linkedInScheduleButton.boundingBox();
const linkedInScheduleButtonClass = await linkedInScheduleButton.getAttribute("class");
const linkedInButtonStyle = await linkedInDownloadButton.evaluate((element) => {
  const style = getComputedStyle(element);
  return {
    borderRadius: style.borderRadius,
    fontSize: style.fontSize
  };
});
await page.getByRole("button", { name: "World Journal" }).click();
const sidebar = await page.locator(".lv-sidebar").boundingBox();
const panels = await page.locator(".newspaper-dispatch-panel").evaluateAll((elements) =>
  elements.map((element) => {
    const rect = element.getBoundingClientRect();
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
  })
);
assert.ok(sidebar, "LinkVault sidebar must remain visible.");
assert.equal(await page.locator(".newspaper-page-header").count(), 0, "Duplicate newspaper page header must stay removed.");
assert.equal(await page.locator(".newspaper-panel-heading").count(), 0, "Dispatch panels must not render separate title rows.");
assert.equal(await page.locator(".newspaper-panel-step").count(), 0, "Dispatch panels must not render numbered steps.");
assert.equal(panels.length, 3, "Newspaper workspace must expose three primary panels.");
assert.ok(panels.every((panel) => Math.abs(panel.y - panels[0].y) <= 1), "Dispatch panel tops are not aligned.");
assert.ok(panels.every((panel) => Math.abs(panel.height - panels[0].height) <= 1), "Dispatch panel bottoms are not aligned.");
assert.ok(panels.every((panel) => panel.width >= 280), "A dispatch panel is too narrow at the approved desktop size.");
const settingsPanel = await page.locator(".newspaper-options").boundingBox();
const schedulePanel = await page.locator(".newspaper-scheduler").boundingBox();
assert.ok(settingsPanel && schedulePanel && settingsPanel.x < schedulePanel.x, "Download settings must render before Schedule.");
const progress = await page.locator(".newspaper-progress-panel").boundingBox();
assert.ok(progress && progress.y > panels[0].y + panels[0].height, "Progress must sit below the three dispatch panels.");
assert.equal(await page.locator(".newspaper-progress-head").getByText("Newspaper", { exact: true }).count(), 1);
const progressSurfaces = await page.locator(".newspaper-progress-panel, .newspaper-progress-table").evaluateAll((elements) =>
  elements.map((element) => ({
    borderWidth: getComputedStyle(element).borderTopWidth,
    background: getComputedStyle(element).backgroundColor
  }))
);
assert.equal(progressSurfaces[0].borderWidth, "0px", "Progress outer wrapper must remain visually flat.");
assert.notEqual(progressSurfaces[1].borderWidth, "0px", "Progress table must remain the single bounded surface.");
const downloadButton = page.getByRole("button", { name: "Download now" });
assert.equal(await downloadButton.count(), 1);
const downloadButtonBox = await downloadButton.boundingBox();
const downloadButtonClass = await downloadButton.getAttribute("class");
const downloadButtonStyle = await downloadButton.evaluate((element) => {
  const style = getComputedStyle(element);
  return {
    borderRadius: style.borderRadius,
    fontSize: style.fontSize
  };
});
assert.equal(Math.round(downloadButtonBox?.height ?? 0), Math.round(linkedInButtonBox?.height ?? 0), "Download now must reuse the LinkedIn course button height.");
assert.equal(downloadButtonClass, linkedInButtonClass, "Download now must reuse the LinkedIn course button classes.");
assert.deepEqual(downloadButtonStyle, linkedInButtonStyle, "Download now must reuse the LinkedIn course button visual style.");
assert.ok((downloadButtonBox?.width ?? 999) < 200, "Download now must reuse the compact LinkedIn course button width.");
const scheduleButton = page.getByRole("button", { name: /Add (daily schedule|another time)/ });
const scheduleButtonBox = await scheduleButton.boundingBox();
assert.equal(Math.round(scheduleButtonBox?.height ?? 0), Math.round(linkedInScheduleButtonBox?.height ?? 0), "Add daily schedule must reuse the LinkedIn schedule button height.");
assert.equal(await scheduleButton.getAttribute("class"), linkedInScheduleButtonClass, "Add daily schedule must reuse the LinkedIn schedule button classes.");
assert.ok((scheduleButtonBox?.width ?? 999) < 200, "Add daily schedule must remain compact.");
assert.equal(await page.locator(".newspaper-options").getByText("Save location", { exact: true }).count(), 1);
await page.setViewportSize({ width: 1400, height: 720 });
const compactPanels = await page.locator(".newspaper-dispatch-panel").evaluateAll((elements) =>
  elements.map((element) => {
    const rect = element.getBoundingClientRect();
    return { top: rect.top, bottom: rect.bottom };
  })
);
const compactProgress = await page.locator(".newspaper-progress-panel").boundingBox();
const compactEditionFooter = await page.locator(".newspaper-edition-footer").boundingBox();
const compactScheduleAction = await scheduleButton.boundingBox();
const compactDownloadAction = await downloadButton.boundingBox();
assert.ok(compactProgress && compactProgress.y >= Math.max(...compactPanels.map((panel) => panel.bottom)) + 10, "Progress must remain below compact dispatch cards.");
assert.ok(compactProgress.height >= 220, "Compact layout must reserve useful height for Progress.");
assert.ok(compactEditionFooter && compactEditionFooter.y + compactEditionFooter.height <= compactPanels[0].bottom, "Edition footer must stay inside its card.");
assert.ok(compactScheduleAction && compactScheduleAction.y + compactScheduleAction.height <= compactPanels[1].bottom, "Schedule action must stay inside its card.");
assert.ok(compactDownloadAction && compactDownloadAction.y + compactDownloadAction.height <= compactPanels[2].bottom, "Download action must stay inside its card.");
await page.getByLabel("Date range").selectOption("last7_days");
assert.equal(await page.getByLabel("System current date").isDisabled(), true, "Last 7 days must disable manual date editing.");
await page.getByRole("tab", { name: "History" }).click();
assert.equal(await page.locator(".newspaper-history-list").count(), 1, "History must remain inside the Schedule panel.");
await page.getByRole("tab", { name: "Daily schedule" }).click();
for (const width of [1920, 1760, 1600, 1451, 1450, 1449, 1366, 1280, 1366, 1449, 1451, 1600, 1920]) {
  await page.setViewportSize({ width, height: 720 });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const resizeState = await page.evaluate(() => ({
    resizing: document.documentElement.dataset.windowResizing,
    viewportWidth: document.documentElement.clientWidth,
    documentWidth: document.documentElement.scrollWidth
  }));
  assert.equal(resizeState.resizing, "true", `Resize guard must activate at ${width}px.`);
  assert.ok(resizeState.documentWidth <= resizeState.viewportWidth + 1, `Page must not overflow horizontally at ${width}px.`);
  const sweepEditions = await page.locator(".newspaper-editions").boundingBox();
  const sweepSettings = await page.locator(".newspaper-options").boundingBox();
  const sweepSchedule = await page.locator(".newspaper-scheduler").boundingBox();
  const sweepProgress = await page.locator(".newspaper-progress-panel").boundingBox();
  const sweepScheduleButton = await scheduleButton.boundingBox();
  const sweepDownloadButton = await downloadButton.boundingBox();
  assert.ok(sweepEditions && sweepSettings && sweepSchedule && sweepProgress, `Dispatch surfaces must render at ${width}px.`);
  assert.ok(sweepSettings.x > sweepEditions.x && sweepSchedule.x > sweepSettings.x, `Card order must stay Editions, Settings, Schedule at ${width}px.`);
  assert.ok(Math.abs(sweepEditions.y - sweepSettings.y) <= 1 && Math.abs(sweepSettings.y - sweepSchedule.y) <= 1, `Cards must stay aligned at ${width}px.`);
  assert.ok(sweepProgress.y >= sweepEditions.y + sweepEditions.height + 10, `Progress must stay below cards at ${width}px.`);
  assert.ok(sweepScheduleButton && sweepScheduleButton.y + sweepScheduleButton.height <= sweepSchedule.y + sweepSchedule.height, `Schedule action must stay contained at ${width}px.`);
  assert.ok(sweepDownloadButton && sweepDownloadButton.y + sweepDownloadButton.height <= sweepSettings.y + sweepSettings.height, `Download action must stay contained at ${width}px.`);
}
await page.waitForTimeout(180);
assert.equal(await page.evaluate(() => document.documentElement.dataset.windowResizing), undefined, "Resize guard must clear after resizing settles.");
await page.getByRole("button", { name: "Newspaper library" }).click();
const librarySearch = await page.getByLabel("Search newspaper library").boundingBox();
const registerArchive = await page.getByRole("button", { name: "Register archive" }).boundingBox();
assert.ok(librarySearch && registerArchive, "Newspaper library controls must render.");
assert.ok(Math.abs(librarySearch.height - registerArchive.height) <= 1, "Library search must match the adjacent button height.");
await page.getByRole("button", { name: "Toggle sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "collapsed");
assert.equal(await page.getByRole("button", { name: "Show sidebar" }).isVisible(), true, "Collapsed Newspaper Library must expose the sidebar reopen button.");
await page.getByRole("button", { name: "Show sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "expanded");
await page.getByRole("button", { name: "Download editions" }).click();
await page.setViewportSize({ width: 1100, height: 900 });
const narrowEditions = await page.locator(".newspaper-editions").boundingBox();
const narrowSettings = await page.locator(".newspaper-options").boundingBox();
const narrowSchedule = await page.locator(".newspaper-scheduler").boundingBox();
assert.ok(narrowEditions && narrowSettings && Math.abs(narrowEditions.y - narrowSettings.y) <= 1, "Editions and settings must share the first responsive row.");
assert.ok(narrowSchedule && narrowSettings && narrowSchedule.y > narrowSettings.y, "Schedule must move below editions and settings at the responsive breakpoint.");
await browser.close();
console.log("Visual geometry verification passed.");
