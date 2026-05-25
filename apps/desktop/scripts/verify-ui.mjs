import { chromium } from "playwright";
import { spawn, spawnSync } from "node:child_process";
import { mkdir } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const outputDir = path.join(root, "output", "playwright");
const preferredPort = Number(process.env.LINKVAULT_UI_PORT ?? 1430);

function assertUi(condition, message) {
  if (!condition) {
    throw new Error(`UI assertion failed: ${message}`);
  }
}

async function isPortFree(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once("error", () => resolve(false));
    server.once("listening", () => {
      server.close(() => resolve(true));
    });
    server.listen(port, "127.0.0.1");
  });
}

async function findFreePort(startPort) {
  for (let port = startPort; port < startPort + 100; port += 1) {
    if (await isPortFree(port)) return port;
  }
  throw new Error(`Could not find a free local port starting at ${startPort}.`);
}

async function waitForServer(url) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw new Error(`Vite server did not become ready at ${url}.`);
}

function startVite(port) {
  return spawn(
    "cmd.exe",
    ["/d", "/s", "/c", "pnpm.cmd", "exec", "vite", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    {
      cwd: root,
      stdio: "ignore",
      windowsHide: true
    }
  );
}

async function openApp(page, baseUrl, preview = "ui-tests") {
  await page.setViewportSize({ width: 1536, height: 1024 });
  await page.goto(baseUrl, { waitUntil: "commit", timeout: 90000 });
  await page.evaluate(() => {
    window.localStorage.removeItem("linkvault.sidebarCollapsed");
    window.localStorage.removeItem("linkvault.sidebarWidth");
  });
  await page.goto(`${baseUrl}/?preview=${encodeURIComponent(preview)}&run=${Date.now()}`, { waitUntil: "commit", timeout: 90000 });
  await page.waitForSelector(".lv-shell", { timeout: 90000 });
  await page.getByLabel("Download folder").fill("C:\\Users\\howard\\Downloads\\LinkVault UI Tests");
  await page.evaluate(() => {
    document.body.tabIndex = -1;
    document.body.focus();
  });
  await page.waitForTimeout(100);
}

async function bodyText(page) {
  return page.locator("body").innerText();
}

async function focusedSignature(page) {
  return page.evaluate(() => {
    const element = document.activeElement;
    if (!element) return "";
    const ariaLabel = element.getAttribute("aria-label");
    if (ariaLabel) return ariaLabel.trim();
    const labelledBy = element.getAttribute("aria-labelledby");
    if (labelledBy) {
      const labelText = labelledBy
        .split(/\s+/)
        .map((id) => document.getElementById(id)?.textContent?.trim())
        .filter(Boolean)
        .join(" ");
      if (labelText) return labelText;
    }
    if (element instanceof HTMLInputElement && element.labels?.length) {
      return Array.from(element.labels)
        .map((label) => label.textContent?.replace(/\s+/g, " ").trim())
        .filter(Boolean)
        .join(" ");
    }
    return element.textContent?.replace(/\s+/g, " ").trim() ?? element.tagName;
  });
}

async function expectNextFocus(page, expectedText) {
  await page.keyboard.press("Tab");
  const actual = await focusedSignature(page);
  assertUi(actual.includes(expectedText), `expected next keyboard focus to include "${expectedText}", saw "${actual}".`);
}

async function clickStartDownload(startButton) {
  await startButton.evaluate((button) => button.click());
}

async function verifyInvalidUrl(page) {
  const urls = page.getByLabel("Course URLs");
  await urls.fill("https://example.com/?next=https://www.linkedin.com/learning/service-desk-fundamentals");
  await page.getByLabel("Download folder").focus();

  await page.getByText("Invalid course URL").waitFor();
  await page.getByText("line 1: expected a linkedin.com/learning course URL").waitFor();

  const text = await bodyText(page);
  assertUi(!text.includes("validated"), "invalid URL should not leave validated course state behind.");
  assertUi(text.includes("No active downloads"), "invalid URL should not create preview or persisted queue rows.");
  assertUi(await page.getByRole("button", { name: /Start Download/ }).isDisabled(), "Start Download should stay guarded after invalid URL.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-invalid-url.png") });
}

async function verifyMultipleUrls(page) {
  const urls = page.getByLabel("Course URLs");
  await urls.fill("\nhttps://www.linkedin.com/learning/first-course\n\nwww.linkedin.com/learning/second-course?trk=share\n");
  await page.getByLabel("Download folder").focus();

  await page.getByText("Course URLs validated").waitFor();
  await page.getByText("2 LinkedIn Learning courses ready to queue.").waitFor();

  const text = await bodyText(page);
  const firstIndex = text.indexOf("https://www.linkedin.com/learning/first-course");
  const secondIndex = text.indexOf("https://www.linkedin.com/learning/second-course");

  assertUi(text.includes("2 validated"), "multiple valid URLs should update the queue header with validated count.");
  assertUi(text.includes("https://www.linkedin.com/learning/first-course"), "first URL should be normalized in the preview row.");
  assertUi(text.includes("https://www.linkedin.com/learning/second-course"), "second URL should be normalized in the preview row.");
  assertUi(firstIndex >= 0 && secondIndex >= 0 && firstIndex < secondIndex, "validated preview rows should preserve URL order.");
  assertUi(await page.getByRole("button", { name: /Start Download/ }).isDisabled(), "Start Download should remain disabled until token/session is available.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-multiple-urls.png") });
}

async function verifySavedTokenReuse(page, baseUrl) {
  await openApp(page, baseUrl, "saved-token-reuse");

  const urls = page.getByLabel("Course URLs");
  const token = page.getByLabel("LinkedIn li_at token");
  const startButton = page.getByRole("button", { name: /Start Download/ });

  await urls.fill("https://www.linkedin.com/learning/first-saved-token-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();
  await token.fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available with a first pasted token.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Queued download processed").waitFor();

  await token.fill("");
  await urls.fill("https://www.linkedin.com/learning/second-saved-token-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").first().waitFor();
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "saved token should allow a later Start Download without re-entering li_at.");

  const text = await bodyText(page);
  const placeholder = await token.getAttribute("placeholder");
  assertUi(placeholder === "••••••••••••••••", "saved token placeholder should make the persisted session state visible without showing the token.");
  assertUi(!text.includes("preview-li-at-token"), "saved-token UI must not expose the pasted token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-saved-token-reuse.png") });
}


async function verifyCourseShapeDrift(page, baseUrl) {
  await openApp(page, baseUrl, "metadata-shape-drift");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  assertUi(await startButton.isDisabled(), "Start Download should be guarded before a token/session exists.");

  await page.getByLabel("Course URLs").fill("https://www.linkedin.com/learning/shape-drift-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();
  assertUi(await startButton.isDisabled(), "valid URLs alone should not enable Start Download without a token/session.");

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available after URL and token are present.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Download processing failed").waitFor();
  await page.getByText("LinkedIn course metadata shape changed").waitFor();

  const text = await bodyText(page);
  assertUi(text.includes("1 failed"), "shape drift should refresh the persisted queue summary to a failed job.");
  assertUi(!text.includes("No active downloads"), "failed shape-drift jobs should stay in the live queue until handled.");
  assertUi(text.includes("Shape Drift Course"), "failed shape-drift job should remain visible for retry or review.");
  assertUi(text.includes("Failed"), "failed shape-drift job should show terminal failed state.");
  assertUi(text.includes("Course metadata fetch or artifact planning failed."), "safe failure event should be visible in activity.");
  assertUi(!text.includes("unsafe raw body"), "shape-drift UI must not expose raw metadata response text.");
  assertUi(!text.includes("do-not-render"), "shape-drift UI must not expose raw secret-like response values.");
  assertUi(!text.includes("preview-li-at-token"), "shape-drift UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-course-shape-drift.png") });
}

async function verifyExercise404(page, baseUrl) {
  await openApp(page, baseUrl, "exercise-404");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill("https://www.linkedin.com/learning/exercise-404-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for the exercise 404 scenario.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Queued download processed").waitFor();
  await page.getByText("2 completed, 1 failed, 0 cancelled.").waitFor();
  await page.getByText("Exercise artifact returned 404 and was skipped.").waitFor();

  const text = await bodyText(page);
  assertUi(text.includes("0 active"), "exercise 404 should clear completed jobs out of the live queue summary.");
  assertUi(text.includes("No active downloads"), "completed exercise-404 jobs should leave no active queue row.");
  assertUi(text.includes("Exercise 404 Course"), "exercise-404 course should remain visible in history.");
  assertUi(text.includes("2 of 3 files, 1 failed"), "history should expose the failed optional exercise count.");
  assertUi(text.includes("Video artifact completed after optional exercise failure."), "video progress should continue after exercise 404.");
  assertUi(text.includes("Subtitle artifact completed after optional exercise failure."), "subtitle progress should continue after exercise 404.");
  assertUi(!text.includes("do-not-render-signed-url"), "exercise 404 UI must not expose signed exercise URLs.");
  assertUi(!text.includes("preview-li-at-token"), "exercise 404 UI must not expose the manual token value.");
  await page.getByRole("button", { name: "Open Folder" }).first().click();
  await page.getByText("Folder opener unavailable in preview").waitFor();

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-exercise-404.png") });
}

async function verifyMultiCourseProgress(page, baseUrl) {
  await openApp(page, baseUrl, "multi-course-progress");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill([
    "https://www.linkedin.com/learning/first-lifecycle-course",
    "https://www.linkedin.com/learning/second-lifecycle-course"
  ].join("\n"));
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();
  await page.getByText("2 LinkedIn Learning courses ready to queue.").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for multiple queued courses.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Queued download processed").waitFor();
  await page.getByText("12 completed, 0 failed, 0 cancelled.").waitFor();
  await page.getByText("Completed one queued course before continuing to the next course.").waitFor();

  const text = await bodyText(page);
  const completedTitles = await page.locator(".completed-row .font-medium").allTextContents();

  assertUi(text.includes("0 active"), "multi-course queue summary should be drained after the batch finishes.");
  assertUi(completedTitles.includes("First Lifecycle Course"), "first lifecycle course should be visible in completed history.");
  assertUi(completedTitles.includes("Second Lifecycle Course"), "second lifecycle course should be visible in completed history.");
  assertUi(
    completedTitles.indexOf("First Lifecycle Course") < completedTitles.indexOf("Second Lifecycle Course"),
    "multiple-course lifecycle should preserve visible completed-history order."
  );
  assertUi(text.includes("Completed"), "completed courses should move into terminal history after the batch drains.");
  assertUi(!text.includes("do-not-render-queue-secret"), "multi-course UI must not expose internal queue-only values.");
  assertUi(!text.includes("preview-li-at-token"), "multi-course UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-multi-course-progress.png") });
}

async function verifyLivePollingProgress(page, baseUrl) {
  await openApp(page, baseUrl, "live-polling-progress");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill([
    "https://www.linkedin.com/learning/live-polling-course",
    "https://www.linkedin.com/learning/queued-after-live-polling-course"
  ].join("\n"));
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for live polling coverage.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("1 active - 1 queued").waitFor({ timeout: 1000 });
  await page.getByText("Live polling course video started.").waitFor({ timeout: 5000 });
  await page.locator("body").getByText("1/6 files").first().waitFor({ timeout: 5000 });
  await page.locator("body").getByText("Live Polling Course").first().waitFor();
  await page.locator("body").getByText("Queued After Live Polling Course").first().waitFor();

  let text = await bodyText(page);
  assertUi(text.includes("1 active - 1 queued"), "live polling should refresh to one active and one queued course before completion.");
  assertUi(text.includes("1/6 files"), "live polling should expose intermediate compact file progress.");

  await page.getByText("Queued download processed").waitFor({ timeout: 5000 });
  await page.getByText("12 completed, 0 failed, 0 cancelled.").waitFor();
  text = await bodyText(page);
  assertUi(text.includes("0 active"), "live polling batch completion should drain the remaining queued course.");
  assertUi(text.includes("Live polling exercise archive extracted."), "live polling should preserve the exercise extraction terminal event.");
  assertUi(!text.includes("do-not-render-live-polling-token"), "live polling UI must not expose token-like preview internals.");
  assertUi(!text.includes("preview-li-at-token"), "live polling UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-live-polling-progress.png") });
}

async function verifyFailedCourseLifecycle(page, baseUrl) {
  await openApp(page, baseUrl, "failed-course-lifecycle");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill([
    "https://www.linkedin.com/learning/first-failed-lifecycle-course",
    "https://www.linkedin.com/learning/second-still-queued-course"
  ].join("\n"));
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();
  await page.getByText("2 LinkedIn Learning courses ready to queue.").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for failed-course lifecycle coverage.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Download processing failed").waitFor();
  await page.getByText("First queued course failed before artifact planning; remaining courses stay queued.").waitFor();

  const text = await bodyText(page);
  assertUi(text.includes("1 queued - 1 failed"), "failed-course lifecycle should show one queued course and one failed course.");
  assertUi(text.includes("Second Still Queued Course"), "remaining course should stay visible in the active queue.");
  assertUi(text.includes("0/4 files"), "remaining queued course should preserve its own compact file plan.");
  assertUi(text.includes("First Failed Lifecycle Course"), "failed course should remain visible in terminal history.");
  assertUi(text.includes("Failed"), "failed course should show terminal failed state.");
  assertUi(await page.getByRole("button", { name: "Retry First Failed Lifecycle Course" }).count() === 1, "failed queue status should expose one inline retry control.");
  assertUi(text.includes("0 files"), "metadata/planning failure should not invent artifact progress.");
  assertUi(!text.includes("do-not-render-failed-course-body"), "failed-course UI must not expose unsafe backend response body.");
  assertUi(!text.includes("do-not-render-failed-course-token"), "failed-course UI must not expose secret-like backend values.");
  assertUi(!text.includes("preview-li-at-token"), "failed-course UI must not expose the manual token value.");
  await page.getByRole("button", { name: "Retry First Failed Lifecycle Course" }).click();
  await page.getByText("Retry queued").waitFor();

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-failed-course-lifecycle.png") });
}

async function verifyRepetitiveArtifactFailureToasts(page, baseUrl) {
  await openApp(page, baseUrl, "repetitive-artifact-failures");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill("https://www.linkedin.com/learning/repeated-artifact-failures-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for repeated artifact failure coverage.");

  await clickStartDownload(startButton);
  await page.getByText("Download queued").waitFor();
  await page.getByText("Queued download processed with issues").waitFor();
  await page.getByText("2 completed, 6 failed, 0 cancelled.").waitFor();
  await page.getByText("6 exercise artifacts failed; details are coalesced in activity.").waitFor();

  const text = await bodyText(page);
  const failureToastCount = await page.locator("[data-sonner-toast]").filter({ hasText: /failed/i }).count();

  assertUi(failureToastCount === 1, `repeated artifact failures should produce one coalesced failure toast, saw ${failureToastCount}.`);
  assertUi(text.includes("2 of 8 files, 6 failed"), "history should show the coalesced repeated failure count.");
  assertUi(text.includes("Video and subtitle artifacts completed despite repeated exercise failures."), "activity should preserve successful artifact context.");
  assertUi(!text.includes("do-not-render-repeated-failure-url"), "repeated failure UI must not expose signed artifact URLs.");
  assertUi(!text.includes("preview-li-at-token"), "repeated failure UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-repetitive-artifact-failures.png") });
}

async function verifyKeyboardNavigation(page, baseUrl) {
  await openApp(page, baseUrl);

  const expectedFocusOrder = [
    "Toggle sidebar",
    "LinkedIn Courses",
    "Generic Video",
    "Tools",
    "History",
    "Open settings",
    "Open help",
    "Course URLs",
    "Download folder",
    "Browse",
    "LinkedIn li_at token",
    "Guide",
    "Clear",
    "Video resolution",
    "Delay seconds",
    "Videos",
    "Exercises",
    "Subtitles"
  ];

  for (const expectedText of expectedFocusOrder) {
    await expectNextFocus(page, expectedText);
  }

  assertUi(await page.getByRole("button", { name: /Start Download/ }).isDisabled(), "keyboard traversal should preserve guarded Start Download state.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-keyboard-navigation.png") });
}

async function verifyPrimitiveOverlays(page, baseUrl) {
  await openApp(page, baseUrl);

  assertUi(await page.locator(".lv-header").count() === 0, "redundant top course/status header should be removed to give the workspace more room.");

  await page.getByLabel("Toggle sidebar").click();
  await page.waitForSelector('.lv-shell[data-sidebar-state="collapsed"]');
  await page.waitForTimeout(240);
  const collapsedSidebar = await page.locator(".lv-sidebar").boundingBox();
  assertUi(collapsedSidebar.x < -150 || collapsedSidebar.width === 0, `collapsed sidebar should move offcanvas, saw x=${collapsedSidebar.x}, width=${collapsedSidebar.width}.`);
  await page.getByLabel("Show sidebar").click();
  await page.waitForSelector('.lv-shell[data-sidebar-state="expanded"]');
  await page.waitForTimeout(240);

  const sidebarBeforeDrag = await page.locator(".lv-sidebar").boundingBox();
  const railBounds = await page.locator(".lv-sidebar-rail").boundingBox();
  assertUi(Boolean(sidebarBeforeDrag), "sidebar should expose a measurable box before resize.");
  assertUi(Boolean(railBounds), "sidebar resize rail should expose a measurable box.");
  await page.mouse.move(railBounds.x + railBounds.width / 2, railBounds.y + railBounds.height / 2);
  await page.mouse.down();
  await page.mouse.move(railBounds.x + railBounds.width / 2 + 44, railBounds.y + railBounds.height / 2, { steps: 6 });
  await page.mouse.up();
  const sidebarAfterDrag = await page.locator(".lv-sidebar").boundingBox();
  assertUi(Boolean(sidebarAfterDrag), "sidebar should expose a measurable box after resize.");
  assertUi(sidebarAfterDrag.width >= sidebarBeforeDrag.width + 35, `sidebar resize rail should drag wider; before=${sidebarBeforeDrag.width}, after=${sidebarAfterDrag.width}.`);

  await page.getByLabel("Open help").click();
  const helpDialog = page.getByRole("dialog", { name: "LinkVault help" });
  await helpDialog.waitFor();
  const helpBounds = await helpDialog.boundingBox();
  const sidebarBounds = await page.locator(".lv-sidebar").boundingBox();
  assertUi(Boolean(helpBounds), "help popover should expose a measurable dialog box.");
  assertUi(Boolean(sidebarBounds), "sidebar should expose a measurable box for overlay positioning checks.");
  assertUi(helpBounds.x >= 8, `help popover should be clamped inside the left viewport edge, saw x=${helpBounds.x}.`);
  assertUi(helpBounds.x + helpBounds.width <= page.viewportSize().width - 8, "help popover should be clamped inside the right viewport edge.");
  assertUi(
    helpBounds.x >= sidebarBounds.x + sidebarBounds.width + 4,
    `sidebar help popover should open to the right of the left rail like Jan sidebar menus; saw help x=${helpBounds.x}, sidebar right=${sidebarBounds.x + sidebarBounds.width}.`
  );
  assertUi(await helpDialog.getAttribute("data-popover-side") === "right", "sidebar help popover should declare right-side placement.");
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector('[role="dialog"][aria-label="LinkVault help"]'));

  await page.getByRole("button", { name: "Guide" }).click();
  const tokenGuide = page.getByRole("dialog", { name: "Find your LinkedIn li_at cookie" });
  await tokenGuide.waitFor();
  const tokenGuideText = await tokenGuide.innerText();
  for (const expected of [
    "Press F12",
    "Application tab",
    "Cookies",
    "li_at"
  ]) {
    assertUi(tokenGuideText.includes(expected), `token guide dialog should include "${expected}".`);
  }
  assertUi(await tokenGuide.locator('img[alt*="Chrome DevTools"]').count() === 1, "token guide dialog should render the instruction image.");
  await page.getByRole("button", { name: "Got it" }).click();
  await page.waitForFunction(() => !document.querySelector('[role="dialog"][aria-modal="true"]'));

  await page.getByLabel("Open settings").click();
  await page.getByRole("dialog", { name: "LinkVault settings" }).waitFor();
  let focused = await focusedSignature(page);
  assertUi(focused.includes("Close LinkVault settings"), `settings dialog should focus its close button, saw "${focused}".`);
  const settingsText = await page.getByRole("dialog", { name: "LinkVault settings" }).innerText();
  for (const expected of [
    "Download defaults",
    "Artifact",
    "LinkedIn session",
    "Application",
    "Browser source",
    "Download videos by default",
    "Plaintext token storage",
    "Jan dark"
  ]) {
    assertUi(settingsText.includes(expected), `settings dialog should include "${expected}".`);
  }
  await page.getByRole("button", { name: "Save settings" }).click();
  const savingButton = page.getByRole("button", { name: "Saving" });
  await savingButton.waitFor();
  assertUi(await savingButton.getAttribute("data-loading") === "true", "saving button should expose an animated loading state.");
  assertUi(await savingButton.locator(".lv-button-spinner").count() === 1, "saving button should render a spinner while preferences persist.");
  await page.getByText("Settings saved").waitFor();
  assertUi(await page.locator("[data-sonner-toast].lv-toast").count() >= 1, "toasts should use the LinkVault themed Sonner class.");
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector('[role="dialog"][aria-modal="true"]'));
  await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Open settings");
  focused = await focusedSignature(page);
  assertUi(focused.includes("Open settings"), `settings dialog should return focus to its trigger, saw "${focused}".`);

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-primitive-overlays.png") });
}

async function verifyBrowseFolderPreviewFallback(page, baseUrl) {
  await openApp(page, baseUrl);

  const folder = page.getByLabel("Download folder");
  const beforeFolder = await folder.inputValue();
  await page.getByRole("button", { name: "Browse" }).click();
  await page.getByText("Folder picker unavailable in preview").waitFor();
  await page.getByText("The native folder picker is available in the Tauri desktop runtime.").waitFor();

  const afterFolder = await folder.inputValue();
  assertUi(afterFolder === beforeFolder, "preview Browse fallback should not corrupt the current folder setting.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-folder-picker-preview.png") });
}

await mkdir(outputDir, { recursive: true });

const port = await findFreePort(preferredPort);
const baseUrl = `http://127.0.0.1:${port}`;
const server = startVite(port);
let browser;

try {
  await waitForServer(baseUrl);
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.addInitScript(() => window.sessionStorage.clear());
  const linkedInRequests = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.hostname === "linkedin.com" || url.hostname.endsWith(".linkedin.com")) {
      linkedInRequests.push(request.url());
    }
  });

  await openApp(page, baseUrl);
  await verifyInvalidUrl(page);
  await verifyMultipleUrls(page);
  await verifySavedTokenReuse(page, baseUrl);
  await verifyCourseShapeDrift(page, baseUrl);
  await verifyExercise404(page, baseUrl);
  await verifyMultiCourseProgress(page, baseUrl);
  await verifyLivePollingProgress(page, baseUrl);
  await verifyFailedCourseLifecycle(page, baseUrl);
  await verifyRepetitiveArtifactFailureToasts(page, baseUrl);
  await verifyKeyboardNavigation(page, baseUrl);
  await verifyPrimitiveOverlays(page, baseUrl);
  await verifyBrowseFolderPreviewFallback(page, baseUrl);

  assertUi(linkedInRequests.length === 0, `browser-preview UI tests must not call LinkedIn: ${linkedInRequests.join(", ")}`);

  process.stdout.write(`LinkVault interactive UI assertions passed on ${baseUrl}\n`);
  process.stdout.write(`Screenshots written to ${outputDir}\n`);
} finally {
  if (browser) {
    await browser.close();
  }
  if (process.platform === "win32" && server.pid) {
    spawnSync("taskkill", ["/pid", String(server.pid), "/t", "/f"], { stdio: "ignore" });
  } else {
    server.kill();
  }
}
