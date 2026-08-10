import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a built LinkVault preview.");

const browser = await chromium.launch({
  channel: process.env.PLAYWRIGHT_CHANNEL || "chrome",
  headless: true
});

try {
  const page = await browser.newPage({ viewport: { width: 1600, height: 960 } });
  await page.addInitScript(() => {
    const callbacks = new Map();
    let callbackId = 1;
    const svgUrl = (label) => `data:image/svg+xml,${encodeURIComponent(
      `<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1600"><rect width="100%" height="100%" fill="#f4f0e6"/><path d="M0 400h1000M0 800h1000M0 1200h1000M250 0v1600M500 0v1600M750 0v1600" stroke="#928b7a"/><text x="28" y="60" fill="#25221d" font-size="30">${label}</text></svg>`
    )}`;
    const item = {
      jobId: "fixture-job",
      editionCode: "CLIP",
      editionName: "Clipping Fixture Edition",
      publicationDate: "2026-08-09",
      status: "completed",
      outputDir: "C:\\fixture",
      pageCount: 5,
      completedCount: 5,
      warning: null,
      updatedAt: 1,
      thumbnailReady: true,
      thumbnailUrl: svgUrl("Edition"),
      thumbnailVersion: "1-1",
      lastPageId: null,
      lastPageIndex: null,
      furthestPageIndex: null,
      readPageCount: 0,
      readingUpdatedAt: null
    };
    const pages = Array.from({ length: 5 }, (_, index) => ({
      id: `fixture-page-${index}`,
      jobId: "fixture-job",
      canonicalIndex: index,
      pageNumber: `A${index + 1}`,
      sectionName: null,
      status: "completed",
      mediaUrl: svgUrl(`Page ${index + 1}`),
      mediaVersion: 3,
      pixelWidth: 1000,
      pixelHeight: 1600,
      finalBytes: 1000,
      error: null
    }));
    window.__NEWSPAPER_CLIPPING_HARNESS__ = true;
    window.__NEWSPAPER_CLIPPING_TEST__ = {
      createRequests: [],
      createdIds: [],
      commandCounts: {},
      createMode: "success",
      modeAttempts: 0,
      manifestVersion: 3
    };
    window.addEventListener("linkvault:newspaper-clipping-created", (event) => {
      window.__NEWSPAPER_CLIPPING_TEST__.createdIds.push(event.detail.clippingId);
    });
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" }
      },
      transformCallback(callback, once = false) {
        const id = callbackId++;
        callbacks.set(id, { callback, once });
        return id;
      },
      unregisterCallback(id) {
        callbacks.delete(id);
      },
      runCallback(id, data) {
        const entry = callbacks.get(id);
        if (!entry) return;
        entry.callback(data);
        if (entry.once) callbacks.delete(id);
      },
      convertFileSrc(path) {
        return path;
      },
      async invoke(command, args = {}) {
        const counts = window.__NEWSPAPER_CLIPPING_TEST__.commandCounts;
        counts[command] = (counts[command] ?? 0) + 1;
        switch (command) {
          case "bootstrap_state":
            return {
              default_resolution: "P720",
              browser_sources: ["Chrome"],
              stores_plaintext_tokens_in_sqlite: false,
              has_saved_token: true,
              saved_download_preferences: null,
              persisted_jobs: [],
              recent_events: [],
              download_history: [],
              download_history_file_path: ""
            };
          case "get_newspaper_library_page":
            return { items: [item], total: 1, offset: 0, limit: 50, revision: 1 };
          case "ensure_newspaper_thumbnail":
            return { status: "ready", thumbnailUrl: item.thumbnailUrl, thumbnailVersion: "1-1", width: 420, height: 176 };
          case "get_newspaper_reader_manifest":
            return pages.map((page) => ({
              ...page,
              mediaVersion: window.__NEWSPAPER_CLIPPING_TEST__.manifestVersion
            }));
          case "save_newspaper_reading_progress":
            return {
              jobId: "fixture-job",
              lastPageId: args.pageId,
              lastPageIndex: pages.findIndex((candidate) => candidate.id === args.pageId),
              furthestPageIndex: 0,
              readPageCount: 1,
              updatedAt: Date.now()
            };
          case "create_newspaper_clipping": {
            const test = window.__NEWSPAPER_CLIPPING_TEST__;
            test.createRequests.push(structuredClone(args.request));
            test.modeAttempts += 1;
            await new Promise((resolve) => setTimeout(resolve, 120));
            const failure = (code, safeMessage, retryable) => ({
              code,
              safeMessage,
              retryable,
              operationId: args.request.operationId
            });
            if (test.createMode === "retryable-once" && test.modeAttempts === 1) {
              throw failure("STAGING_WRITE_FAILED", "The snapshot folder is temporarily unavailable.", true);
            }
            if (test.createMode === "stale-once" && test.modeAttempts === 1) {
              test.manifestVersion = 4;
              throw failure("SOURCE_MEDIA_STALE", "The newspaper page changed.", false);
            }
            if (test.createMode === "too-small") {
              throw failure("CROP_TOO_SMALL", "Select a larger area.", false);
            }
            if (test.createMode === "security") {
              throw failure("SOURCE_SECURITY_FAILED", "The newspaper source location is not trusted.", false);
            }
            return {
              clippingId: args.request.operationId,
              title: "Clipping Fixture Edition · 2026-08-09 · A1",
              editionCode: "CLIP",
              editionName: "Clipping Fixture Edition",
              publicationDate: "2026-08-09",
              pageNumber: "A1",
              imageUrl: "http://newspaper-media.localhost/clipping/test?v=1",
              assetVersion: 1,
              assetWidth: 500,
              assetHeight: 800,
              assetByteCount: 100,
              revision: 1,
              createdAt: Date.now()
            };
          }
          case "get_newspaper_activity_snapshot":
            return { jobs: [], batches: [], schedules: [], hasLiveActivity: false, revision: 1 };
          case "refresh_newspaper_catalog":
          case "process_newspaper_queue":
          case "process_newspaper_optimization_queue":
            return [];
          case "plugin:event|listen":
            return 1;
          case "plugin:event|unlisten":
            return null;
          default:
            return null;
        }
      }
    };
  });

  const consoleErrors = [];
  const failedResources = [];
  page.on("console", (message) => {
    if (message.type() === "error") {
      const location = message.location();
      if (location.url && new URL(location.url).pathname === "/favicon.ico") return;
      consoleErrors.push(`${message.text()} @ ${location.url || "unknown"}:${location.lineNumber ?? 0}`);
    }
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  page.on("response", (response) => {
    if (response.status() >= 400) failedResources.push(`${response.status()} ${response.url()}`);
  });

  await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Newspaper library" }).click();
  await page.locator(".newspaper-library-open").click();
  const reader = page.locator(".newspaper-reader");
  const canvas = page.locator('[data-testid="newspaper-reader-scroll"]');
  const image = page.locator('[data-page-id="fixture-page-0"] [data-testid="newspaper-reader-page-image"]');
  await image.waitFor();
  const setCreateMode = async (mode) => {
    await page.evaluate((nextMode) => {
      const test = window.__NEWSPAPER_CLIPPING_TEST__;
      test.createMode = nextMode;
      test.modeAttempts = 0;
      test.createRequests = [];
      test.createdIds = [];
    }, mode);
  };
  const drawFixtureSelection = async () => {
    await canvas.focus();
    await page.keyboard.press("c");
    assert.equal(await reader.getAttribute("data-clipping-mode"), "selecting");
    const currentBox = await image.boundingBox();
    assert.ok(currentBox, "Fixture page is not mounted for selection");
    const from = {
      x: currentBox.x + currentBox.width * .2,
      y: currentBox.y + Math.min(currentBox.height * .2, 260)
    };
    const to = {
      x: currentBox.x + currentBox.width * .7,
      y: currentBox.y + Math.min(currentBox.height * .45, 620)
    };
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 8 });
    await page.mouse.up();
    await page.waitForFunction(() => document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "confirming");
  };
  await page.getByLabel("Select newspaper page").focus();
  await page.keyboard.press("c");
  assert.equal(await reader.getAttribute("data-clipping-mode"), "browse", "C inside a select toggled Clip mode");
  await canvas.focus();
  await page.keyboard.press("Control+c");
  assert.equal(await reader.getAttribute("data-clipping-mode"), "browse", "Ctrl+C toggled Clip mode");

  await page.getByTestId("newspaper-reader-clip").click();
  assert.equal(await reader.getAttribute("data-clipping-mode"), "selecting");
  assert.equal(await image.evaluate((node) => getComputedStyle(node).cursor), "crosshair");
  const box = await image.boundingBox();
  assert.ok(box, "Reader fixture image has no bounds");
  const before = await canvas.evaluate((node) => ({ left: node.scrollLeft, top: node.scrollTop }));
  const start = { x: box.x + box.width * .2, y: box.y + Math.min(box.height * .2, 260) };
  const end = { x: box.x + box.width * .7, y: box.y + Math.min(box.height * .45, 620) };
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(end.x, end.y, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(120);
  if (await page.getByTestId("newspaper-clipping-confirm").count() === 0) {
    const diagnostic = await page.evaluate(() => ({
      mode: document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode"),
      announcement: document.querySelector(".newspaper-reader .sr-only[aria-live]")?.textContent,
      selectionCount: document.querySelectorAll('[data-testid="newspaper-clipping-selection"]').length,
      imageComplete: document.querySelector('[data-testid="newspaper-reader-page-image"]')?.complete,
      activePage: document.querySelector('[aria-label="Select newspaper page"]')?.value
    }));
    throw new Error(`Clipping confirmation did not render: ${JSON.stringify(diagnostic)}`);
  }
  await page.getByTestId("newspaper-clipping-confirm").waitFor();
  assert.equal(await reader.getAttribute("data-clipping-mode"), "confirming");
  assert.equal(await page.getByTestId("newspaper-clipping-save").evaluate((node) => node === document.activeElement), true);
  await page.keyboard.press("Tab");
  assert.equal(await page.getByTestId("newspaper-clipping-redraw").evaluate((node) => node === document.activeElement), true);
  await page.keyboard.press("Tab");
  assert.equal(await page.getByTestId("newspaper-clipping-cancel").evaluate((node) => node === document.activeElement), true);
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.getByTestId("newspaper-clipping-save").evaluate((node) => node === document.activeElement), true);
  assert.deepEqual(
    await canvas.evaluate((node) => ({ left: node.scrollLeft, top: node.scrollTop })),
    before,
    "Clipping drag changed reader scroll position"
  );
  assert.equal(await page.getByLabel("Select newspaper page").isDisabled(), true);
  assert.ok(Number(await reader.getAttribute("data-mounted-page-images")) <= 3);
  const forwardStyle = await page.getByTestId("newspaper-clipping-selection").locator(".newspaper-clipping-selection").evaluate((node) => ({
    left: node.style.left,
    top: node.style.top,
    width: node.style.width,
    height: node.style.height
  }));
  assert.equal(await page.getByLabel("Newspaper page tone").isDisabled(), true);
  assert.equal(await page.getByRole("slider", { name: "Reader zoom" }).isDisabled(), true);
  await page.mouse.wheel(0, 600);
  await page.waitForTimeout(80);
  assert.deepEqual(
    await canvas.evaluate((node) => ({ left: node.scrollLeft, top: node.scrollTop })),
    before,
    "Confirmation wheel changed reader scroll position"
  );
  await page.setViewportSize({ width: 1500, height: 900 });
  await page.waitForTimeout(120);
  const resizedStyle = await page.getByTestId("newspaper-clipping-selection").locator(".newspaper-clipping-selection").evaluate((node) => ({
    left: node.style.left,
    top: node.style.top,
    width: node.style.width,
    height: node.style.height
  }));
  assert.deepEqual(resizedStyle, forwardStyle, "Viewport resize changed frozen normalized geometry");
  assert.equal(await reader.getAttribute("data-clipping-mode"), "confirming");
  await page.setViewportSize({ width: 1600, height: 960 });
  await page.waitForTimeout(120);

  await page.getByTestId("newspaper-clipping-save").dblclick();
  await page.waitForFunction(() => document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "browse");
  assert.deepEqual(
    await canvas.evaluate((node) => ({ left: node.scrollLeft, top: node.scrollTop })),
    before,
    "Saving a clipping changed reader scroll position"
  );
  const captured = await page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__);
  assert.equal(captured.createRequests.length, 1, "Duplicate Save invoked create more than once");
  assert.equal(captured.createdIds.length, 1, "Create callback was not delivered exactly once");
  assert.equal(captured.createRequests[0].pageId, "fixture-page-0");
  assert.equal(captured.createRequests[0].expectedMediaVersion, 3);
  assert.match(captured.createRequests[0].operationId, /^[0-9a-f-]{36}$/);

  await canvas.focus();
  await page.keyboard.press("c");
  const secondBox = await image.boundingBox();
  assert.ok(secondBox);
  const reverseStart = {
    x: secondBox.x + secondBox.width * .7,
    y: secondBox.y + Math.min(secondBox.height * .45, 620)
  };
  const reverseEnd = {
    x: secondBox.x + secondBox.width * .2,
    y: secondBox.y + Math.min(secondBox.height * .2, 260)
  };
  await page.mouse.move(reverseStart.x, reverseStart.y);
  await page.mouse.down();
  await page.mouse.move(reverseEnd.x, reverseEnd.y, { steps: 8 });
  await page.mouse.up();
  if (await page.getByTestId("newspaper-clipping-confirm").count() === 0) {
    const diagnostic = await page.evaluate(() => ({
      mode: document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode"),
      announcement: document.querySelector(".newspaper-reader .sr-only[aria-live]")?.textContent,
      selectionCount: document.querySelectorAll('[data-testid="newspaper-clipping-selection"]').length,
      imageComplete: document.querySelector('[data-testid="newspaper-reader-page-image"]')?.complete,
      activePage: document.querySelector('[aria-label="Select newspaper page"]')?.value
    }));
    throw new Error(`Reverse-drag confirmation did not render: ${JSON.stringify(diagnostic)}`);
  }
  await page.getByTestId("newspaper-clipping-confirm").waitFor();
  const reverseStyle = await page.getByTestId("newspaper-clipping-selection").locator(".newspaper-clipping-selection").evaluate((node) => ({
    left: node.style.left,
    top: node.style.top,
    width: node.style.width,
    height: node.style.height
  }));
  assert.deepEqual(reverseStyle, forwardStyle, "Reverse drag changed normalized geometry");
  await page.keyboard.press("Escape");
  assert.equal(await reader.getAttribute("data-clipping-mode"), "browse");
  assert.equal(await reader.count(), 1, "Escape cancellation also closed the Reader");

  await setCreateMode("retryable-once");
  await drawFixtureSelection();
  await page.getByTestId("newspaper-clipping-save").click();
  await page.waitForFunction(() => (
    window.__NEWSPAPER_CLIPPING_TEST__.createRequests.length === 1
    && document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "confirming"
  ));
  assert.match(await page.getByRole("alert").textContent(), /temporarily unavailable/i);
  const retryOperationId = await page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__.createRequests[0].operationId);
  await page.getByTestId("newspaper-clipping-save").click();
  await page.waitForFunction(() => document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "browse");
  const retryRequests = await page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__.createRequests);
  assert.equal(retryRequests.length, 2);
  assert.equal(retryRequests[1].operationId, retryOperationId, "Retry generated a new operation ID");

  await setCreateMode("stale-once");
  await drawFixtureSelection();
  const staleOperationId = await page.getByTestId("newspaper-clipping-save").click().then(async () => {
    await page.waitForFunction(() => (
      window.__NEWSPAPER_CLIPPING_TEST__.createRequests.length === 1
      && document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "confirming"
    ));
    return page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__.createRequests[0].operationId);
  });
  assert.equal(await page.getByTestId("newspaper-clipping-save").isDisabled(), true);
  assert.equal(await page.getByTestId("newspaper-clipping-redraw").evaluate((node) => node === document.activeElement), true);
  assert.match(await page.getByRole("alert").textContent(), /click redraw before saving again/i);
  await page.getByTestId("newspaper-clipping-redraw").click();
  assert.equal(await reader.getAttribute("data-clipping-mode"), "selecting");
  await page.keyboard.press("Escape");

  await setCreateMode("too-small");
  await drawFixtureSelection();
  await page.getByTestId("newspaper-clipping-save").click();
  await page.waitForFunction(() => (
    window.__NEWSPAPER_CLIPPING_TEST__.createRequests.length === 1
    && document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "confirming"
  ));
  assert.match(await page.getByRole("alert").textContent(), /larger area/i);
  const tooSmallFirstId = await page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__.createRequests[0].operationId);
  await page.getByTestId("newspaper-clipping-save").click();
  await page.waitForFunction(() => (
    window.__NEWSPAPER_CLIPPING_TEST__.createRequests.length === 2
    && document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "confirming"
  ));
  const tooSmallSecondId = await page.evaluate(() => window.__NEWSPAPER_CLIPPING_TEST__.createRequests[1].operationId);
  assert.notEqual(tooSmallSecondId, tooSmallFirstId, "Known no-create failure retained the old operation ID");
  await page.keyboard.press("Escape");
  assert.equal(await reader.getAttribute("data-clipping-mode"), "browse");

  await setCreateMode("security");
  await drawFixtureSelection();
  await page.getByTestId("newspaper-clipping-save").click();
  await page.waitForFunction(() => document.querySelector(".newspaper-reader")?.getAttribute("data-clipping-mode") === "browse");
  assert.ok(await page.getByText("The newspaper source location is not trusted.").count() >= 1);
  assert.notEqual(staleOperationId, retryOperationId, "Separate create attempts reused an operation ID");

  await setCreateMode("success");
  await canvas.focus();
  await page.keyboard.press("c");
  const blurBox = await image.boundingBox();
  assert.ok(blurBox);
  await page.mouse.move(blurBox.x + blurBox.width * .2, blurBox.y + 160);
  await page.mouse.down();
  await page.mouse.move(blurBox.x + blurBox.width * .5, blurBox.y + 360, { steps: 4 });
  assert.equal(await reader.getAttribute("data-clipping-mode"), "drawing");
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.mouse.up();
  assert.equal(await reader.getAttribute("data-clipping-mode"), "browse", "Window blur retained a drawing lock");
  assert.deepEqual(
    consoleErrors,
    [],
    `Browser console/page errors: ${consoleErrors.join(" | ")}; failed resources: ${failedResources.join(" | ")}`
  );

  console.log("newspaper clipping browser verification passed");
} finally {
  await browser.close();
}
