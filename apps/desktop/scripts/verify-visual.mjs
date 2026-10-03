import assert from "node:assert/strict";
import { chromium } from "playwright";

const url = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(url, "Set LINKVAULT_PREVIEW_URL to a running local LinkedVault preview.");
const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1720, height: 960 } });
await page.goto(url);
await page.waitForFunction(() => Boolean(document.documentElement.dataset.theme));
await page.getByRole("button", { name: "World Journal", exact: true }).click();
await page.locator(".newspaper-download").waitFor({ state: "visible" });
assert.ok(await page.locator(".lv-sidebar").isVisible(), "LinkedVault sidebar must remain visible.");
assert.equal(await page.locator(".newspaper-page-header").count(), 0, "Duplicate newspaper page header must stay removed.");
assert.equal(await page.locator(".newspaper-panel-heading, .newspaper-panel-step").count(), 0, "Download groups must not add title or numbered-step rows.");

async function checkNewspaperGeometry(width) {
  const geometry = await page.locator(".newspaper-download").evaluate((root) => {
    const box = (element) => {
      const rect = element.getBoundingClientRect();
      return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, width: rect.width, height: rect.height };
    };
    const stage = root.querySelector(".newspaper-search-stage");
    const groups = [".newspaper-editions", ".newspaper-control-cluster", ".newspaper-options-row", ".newspaper-action-row"];
    return {
      viewportWidth: document.documentElement.clientWidth,
      documentWidth: document.documentElement.scrollWidth,
      stage: stage && box(stage),
      queue: root.querySelector(".newspaper-queue-panel") && box(root.querySelector(".newspaper-queue-panel")),
      groups: groups.map((selector) => {
        const element = root.querySelector(selector);
        return element && {
          selector, box: box(element),
          controls: Array.from(element.querySelectorAll('button, input:not([type="checkbox"]), select')).map((control) => {
            const rect = box(control);
            const hit = document.elementFromPoint((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            return { box: rect, label: control.getAttribute("aria-label") || control.textContent.trim() || control.tagName,
              reachable: Boolean(hit && (hit === control || control.contains(hit))), disabled: control.disabled };
          })
        };
      }),
      footer: root.querySelector(".newspaper-edition-footer") && box(root.querySelector(".newspaper-edition-footer"))
    };
  });
  const contains = (outer, inner) => inner.left >= outer.left - 1 && inner.right <= outer.right + 1 && inner.top >= outer.top - 1 && inner.bottom <= outer.bottom + 1;
  assert.ok(geometry.documentWidth <= geometry.viewportWidth + 1, `Page must not overflow horizontally at ${width}px.`);
  assert.ok(geometry.stage && geometry.queue && geometry.groups.every(Boolean), `All newspaper groups must render at ${width}px.`);
  for (const [index, group] of geometry.groups.entries()) {
    assert.ok(group.box.width > 0 && group.box.height > 0 && contains(geometry.stage, group.box), `${group.selector} must remain inside the stage at ${width}px.`);
    if (index > 0) assert.ok(group.box.top >= geometry.groups[index - 1].box.bottom + 8, `Groups must remain ordered without overlap at ${width}px.`);
    for (const control of group.controls) {
      assert.ok(control.box.width > 0 && control.box.height > 0 && contains(group.box, control.box), `${control.label} must remain inside ${group.selector} at ${width}px.`);
      assert.ok(control.disabled || control.reachable, `${control.label} must accept a center hit at ${width}px.`);
    }
    for (let first = 0; first < group.controls.length; first++) {
      for (let second = first + 1; second < group.controls.length; second++) {
        const a = group.controls[first].box, b = group.controls[second].box;
        assert.ok(Math.min(a.right, b.right) - Math.max(a.left, b.left) <= 1 || Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) <= 1,
          `Controls must not overlap in ${group.selector} at ${width}px.`);
      }
    }
  }
  assert.ok(geometry.footer && contains(geometry.groups[0].box, geometry.footer), `Edition footer must remain inside its group at ${width}px.`);
  assert.ok(geometry.queue.top >= geometry.stage.bottom + 10, `Queue must remain below download controls at ${width}px.`);
}

const downloadButton = page.getByRole("button", { name: "Download", exact: true });
const scheduleButton = page.getByRole("button", { name: "Add schedule", exact: true });
assert.equal(await downloadButton.count(), 1);
assert.equal(await scheduleButton.count(), 1);
for (const button of [downloadButton, scheduleButton]) {
  const box = await button.boundingBox();
  assert.ok(box && Math.abs(box.height - 32) <= 1 && box.width < 200, "Download actions must retain compact 32px controls.");
}
assert.ok(await page.getByRole("button", { name: "Browse newspaper folder", exact: true }).isVisible(), "Folder picker must remain accessible.");
await checkNewspaperGeometry(1720);
await page.locator(".newspaper-option-date select").selectOption("last7_days");
assert.equal(await page.getByLabel("System current date").isDisabled(), true, "Last 7 days must disable manual date editing.");
assert.equal(await page.locator(".newspaper-schedule-panel, .newspaper-history-list").count(), 0, "Schedules/history must remain integrated with the queue.");
assert.ok(await page.getByRole("button", { name: /Queue/i }).count() >= 1, "Queue tab must remain for schedules and pending jobs.");
assert.ok(await page.getByRole("button", { name: /Completed/i }).count() >= 1, "Completed tab must remain for finished downloads.");
for (const width of [1920, 1760, 1600, 1451, 1450, 1449, 1400, 1366, 1280, 1366, 1449, 1451, 1600, 1920]) {
  await page.setViewportSize({ width, height: 720 });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await page.evaluate(() => document.documentElement.dataset.windowResizing), "true", `Resize guard must activate at ${width}px.`);
  await checkNewspaperGeometry(width);
}
await page.waitForTimeout(180);
assert.equal(await page.evaluate(() => document.documentElement.dataset.windowResizing), undefined, "Resize guard must clear after resizing settles.");
await page.getByRole("button", { name: "Newspaper library", exact: true }).click();
const librarySearch = await page.getByLabel("Search newspaper library").boundingBox();
const libraryKindFilter = await page.getByLabel("Filter newspaper kind").boundingBox();
assert.ok(librarySearch && libraryKindFilter, "Newspaper library controls must render.");
assert.ok(Math.abs(librarySearch.height - libraryKindFilter.height) <= 1, "Library search must match the adjacent filter height.");
await page.getByRole("button", { name: "Toggle sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "collapsed");
assert.equal(await page.getByRole("button", { name: "Show sidebar" }).isVisible(), true, "Collapsed Newspaper Library must expose the sidebar reopen button.");
await page.getByRole("button", { name: "Show sidebar" }).click();
assert.equal(await page.locator(".lv-shell").getAttribute("data-sidebar-state"), "expanded");
await page.getByRole("button", { name: "Download editions", exact: true }).click();
await page.locator(".newspaper-download").waitFor({ state: "visible" });
await page.setViewportSize({ width: 1100, height: 900 });
await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
await checkNewspaperGeometry(1100);

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
