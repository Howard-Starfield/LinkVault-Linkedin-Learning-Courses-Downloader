import assert from "node:assert/strict";
import { chromium } from "playwright";

const url = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(url, "Set LINKVAULT_PREVIEW_URL to a running local LinkedVault preview.");
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1720, height: 960 } });
await page.goto(url);
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));
const linkedInDownloadButton = page.getByRole("button", { name: "Download", exact: true });
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
assert.ok(sidebar, "LinkedVault sidebar must remain visible.");
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
const downloadButton = page.getByRole("button", { name: "Download", exact: true });
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
assert.equal(Math.round(downloadButtonBox?.height ?? 0), Math.round(linkedInButtonBox?.height ?? 0), "Download must reuse the LinkedIn course button height.");
assert.equal(downloadButtonClass, linkedInButtonClass, "Download must reuse the LinkedIn course button classes.");
assert.deepEqual(downloadButtonStyle, linkedInButtonStyle, "Download must reuse the LinkedIn course button visual style.");
assert.ok((downloadButtonBox?.width ?? 999) < 200, "Download must reuse the compact LinkedIn course button width.");
const scheduleButton = page.getByRole("button", { name: "Add schedule", exact: true });
const scheduleButtonBox = await scheduleButton.boundingBox();
assert.equal(Math.round(scheduleButtonBox?.height ?? 0), Math.round(linkedInScheduleButtonBox?.height ?? 0), "Add schedule must reuse the LinkedIn schedule button height.");
assert.equal(await scheduleButton.getAttribute("class"), linkedInScheduleButtonClass, "Add schedule must reuse the LinkedIn schedule button classes.");
assert.ok((scheduleButtonBox?.width ?? 999) < 200, "Add schedule must remain compact.");
assert.ok(Math.abs((scheduleButtonBox?.y ?? 0) - (downloadButtonBox?.y ?? 999)) <= 1, "Add schedule and Download must share one action row.");
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
assert.ok(compactScheduleAction && compactScheduleAction.y + compactScheduleAction.height <= compactPanels[2].bottom, "Schedule action must stay inside Download settings.");
assert.ok(compactDownloadAction && compactDownloadAction.y + compactDownloadAction.height <= compactPanels[2].bottom, "Download action must stay inside its card.");
await page.locator(".newspaper-setting-field select").selectOption("last7_days");
assert.equal(await page.getByLabel("System current date").isDisabled(), true, "Last 7 days must disable manual date editing.");
assert.equal(await page.locator(".newspaper-schedule-panel").count(), 0, "Separate Schedule/History panel must stay removed.");
assert.equal(await page.locator(".newspaper-history-list").count(), 0, "History must not render as a separate list panel.");
assert.ok(await page.getByRole("button", { name: /Queue/i }).count() >= 1, "Queue tab must remain for schedules and pending jobs.");
assert.ok(await page.getByRole("button", { name: /Completed/i }).count() >= 1, "Completed tab must remain for finished downloads.");
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
  assert.ok(sweepScheduleButton && sweepScheduleButton.y + sweepScheduleButton.height <= sweepSettings.y + sweepSettings.height, `Schedule action must stay inside Download settings at ${width}px.`);
  assert.ok(sweepDownloadButton && sweepDownloadButton.y + sweepDownloadButton.height <= sweepSettings.y + sweepSettings.height, `Download action must stay contained at ${width}px.`);
}
await page.waitForTimeout(180);
assert.equal(await page.evaluate(() => document.documentElement.dataset.windowResizing), undefined, "Resize guard must clear after resizing settles.");
await page.getByRole("button", { name: "Newspaper library" }).click();
const librarySearch = await page.getByLabel("Search newspaper library").boundingBox();
const libraryKindFilter = await page.getByLabel("Filter newspaper kind").boundingBox();
assert.ok(librarySearch && libraryKindFilter, "Newspaper library controls must render.");
assert.ok(Math.abs(librarySearch.height - libraryKindFilter.height) <= 1, "Library search must match the adjacent filter height.");
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

// =============================================================================
// Responsive Layout Hardening Tests (PRD: frontend-responsive-layout-hardening)
// =============================================================================

console.log("\n🔍 Running responsive layout hardening tests...");

// Test 1: Brand logo invariance across sidebar widths
console.log("  Testing brand logo invariance...");
await page.setViewportSize({ width: 1720, height: 960 });
await page.waitForTimeout(100);

const brandLogoSelector = ".lv-brand-wordmark";
const sidebarWidthsToTest = [208, 220, 320];
let baselineBrandBox = null;

for (const targetWidth of sidebarWidthsToTest) {
  // Set sidebar width via localStorage and reload to apply
  await page.evaluate((width) => {
    window.localStorage.setItem("linkvault.sidebarWidth", String(width));
  }, targetWidth);
  await page.reload();
  await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));
  await page.waitForTimeout(200);
  
  const brandBox = await page.locator(brandLogoSelector).boundingBox();
  assert.ok(brandBox, `Brand logo must render at sidebar width ${targetWidth}px`);
  
  if (!baselineBrandBox) {
    baselineBrandBox = brandBox;
  } else {
    // Brand size should be invariant within 1 CSS pixel
    const widthDiff = Math.abs(brandBox.width - baselineBrandBox.width);
    const heightDiff = Math.abs(brandBox.height - baselineBrandBox.height);
    assert.ok(widthDiff <= 1, `Brand logo width varied by ${widthDiff}px at sidebar width ${targetWidth}px (max 1px allowed)`);
    assert.ok(heightDiff <= 1, `Brand logo height varied by ${heightDiff}px at sidebar width ${targetWidth}px (max 1px allowed)`);
  }
}

// Restore default sidebar width
await page.evaluate(() => {
  window.localStorage.setItem("linkvault.sidebarWidth", "220");
});
await page.reload();
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));

// Test 2: No horizontal overflow at native floor (1280x720) with max sidebar
console.log("  Testing 1280x720 native floor with max sidebar...");
await page.evaluate(() => {
  window.localStorage.setItem("linkvault.sidebarWidth", "320");
});
await page.reload();
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));
await page.setViewportSize({ width: 1280, height: 720 });
await page.waitForTimeout(200);

const overflowCheck = await page.evaluate(() => ({
  scrollWidth: document.documentElement.scrollWidth,
  clientWidth: document.documentElement.clientWidth
}));
assert.ok(overflowCheck.scrollWidth <= overflowCheck.clientWidth + 1, 
  `Document must not overflow horizontally at 1280x720 with 320px sidebar (scrollWidth=${overflowCheck.scrollWidth}, clientWidth=${overflowCheck.clientWidth})`);

// Check that main content is within bounds
const mainBounds = await page.locator(".lv-main").boundingBox();
const shellBounds = await page.locator(".lv-shell").boundingBox();
assert.ok(mainBounds && shellBounds, "Main and shell elements must render");
assert.ok(mainBounds.x >= shellBounds.x, "Main must be within shell bounds");
assert.ok(mainBounds.x + mainBounds.width <= shellBounds.x + shellBounds.width + 1, 
  "Main must not exceed shell width");

// Test 3: Viewport sweep with overflow checks
console.log("  Testing viewport sweep for overflow...");
const viewportsToTest = [
  { width: 1280, height: 720 },
  { width: 1366, height: 768 },
  { width: 1400, height: 720 },
  { width: 1600, height: 900 },
  { width: 1720, height: 960 },
  { width: 1920, height: 1080 }
];

for (const viewport of viewportsToTest) {
  await page.setViewportSize(viewport);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  
  const sweepOverflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth
  }));
  assert.ok(sweepOverflow.scrollWidth <= sweepOverflow.clientWidth + 1, 
    `No horizontal overflow at ${viewport.width}x${viewport.height} (scrollWidth=${sweepOverflow.scrollWidth}, clientWidth=${sweepOverflow.clientWidth})`);
}

// Test 4: Sidebar collapse/reopen after resize
console.log("  Testing sidebar collapse/reopen after resize...");
await page.setViewportSize({ width: 1720, height: 960 });
await page.evaluate(() => {
  window.localStorage.setItem("linkvault.sidebarWidth", "220");
});
await page.reload();
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));

// Collapse sidebar
await page.getByRole("button", { name: "Toggle sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "collapsed");

// Resize while collapsed
await page.setViewportSize({ width: 1400, height: 720 });
await page.waitForTimeout(100);

// Reopen sidebar
await page.getByRole("button", { name: "Show sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "expanded");

// Verify sidebar is visible and functional
const reopenedSidebar = await page.locator(".lv-sidebar").boundingBox();
assert.ok(reopenedSidebar && reopenedSidebar.width > 0, "Sidebar must be visible after reopen");

// Test 5: Keyboard resize parity
console.log("  Testing keyboard resize...");
await page.setViewportSize({ width: 1720, height: 960 });
await page.evaluate(() => {
  window.localStorage.setItem("linkvault.sidebarWidth", "220");
});
await page.reload();
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));

// Focus the separator
const separator = page.locator('[role="separator"]');
await separator.focus();

// Test ArrowRight (should increase width)
const initialWidth = await page.evaluate(() => {
  return parseFloat(getComputedStyle(document.querySelector(".lv-shell")).getPropertyValue("--sidebar-width"));
});

await separator.press("ArrowRight");
await page.waitForTimeout(50);
const afterArrowRight = await page.evaluate(() => {
  return parseFloat(getComputedStyle(document.querySelector(".lv-shell")).getPropertyValue("--sidebar-width"));
});
assert.ok(afterArrowRight > initialWidth, "ArrowRight should increase sidebar width");

// Test Home (should go to minimum)
await separator.press("Home");
await page.waitForTimeout(50);
const afterHome = await page.evaluate(() => {
  return parseFloat(getComputedStyle(document.querySelector(".lv-shell")).getPropertyValue("--sidebar-width"));
});
assert.ok(Math.abs(afterHome - 208) <= 1, `Home should set sidebar to minimum width (got ${afterHome}, expected ~208)`);

// Test End (should go to maximum)
await separator.press("End");
await page.waitForTimeout(50);
const afterEnd = await page.evaluate(() => {
  return parseFloat(getComputedStyle(document.querySelector(".lv-shell")).getPropertyValue("--sidebar-width"));
});
assert.ok(Math.abs(afterEnd - 320) <= 1, `End should set sidebar to maximum width (got ${afterEnd}, expected ~320)`);

console.log("✅ Responsive layout hardening tests passed.");

await browser.close();
console.log("\n✅ Visual geometry verification passed.");
