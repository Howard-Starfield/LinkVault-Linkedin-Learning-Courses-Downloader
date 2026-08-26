import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a built LinkedVault preview.");

const browser = await chromium.launch({
  channel: process.env.PLAYWRIGHT_CHANNEL || "chrome",
  headless: true
});

const editionCounts = process.env.LINKVAULT_NEWSPAPER_PROFILE_COUNTS
  ? process.env.LINKVAULT_NEWSPAPER_PROFILE_COUNTS.split(",").map(Number).filter(Number.isFinite)
  : [8, 50, 500];
const profiles = [];
try {
  for (const editionCount of editionCounts) {
    console.log(`Profiling ${editionCount} newspaper editions...`);
    const page = await browser.newPage({ viewport: { width: 1720, height: 960 } });
    await page.addInitScript(({ count }) => {
      const callbacks = new Map();
      let callbackId = 1;
      let lastSavedPageId = null;
      const viewedPageIds = new Set();
      const svgUrl = (width, height, label) =>
        `data:image/svg+xml,${encodeURIComponent(
          `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect width="100%" height="100%" fill="#f4f0e6"/><text x="24" y="48" fill="#25221d" font-size="24">${label}</text></svg>`
        )}`;
      const items = Array.from({ length: count }, (_, index) => ({
        jobId: `fixture-job-${index}`,
        editionCode: `E${String(index).padStart(3, "0")}`,
        editionName: `Fixture Edition ${index + 1}`,
        publicationDate: `2026-07-${String((index % 25) + 1).padStart(2, "0")}`,
        status: "completed",
        outputDir: "C:\\fixture",
        pageCount: 38,
        completedCount: 38,
        warning: null,
        updatedAt: 1_753_500_000 + index,
        thumbnailReady: true,
        thumbnailUrl: svgUrl(420, 176, `Edition ${index + 1}`),
        thumbnailVersion: "1-1",
        lastPageId: index === 0 ? "fixture-page-9" : null,
        lastPageIndex: index === 0 ? 9 : null,
        furthestPageIndex: index === 0 ? 12 : null,
        readPageCount: index === 0 ? 1 : 0,
        readingUpdatedAt: index === 0 ? 1_753_500_000 : null
      }));
      const pages = Array.from({ length: 38 }, (_, index) => ({
        id: `fixture-page-${index}`,
        jobId: "fixture-job-0",
        canonicalIndex: index,
        pageNumber: `A${String(index + 1).padStart(2, "0")}`,
        sectionName: null,
        status: "completed",
        mediaUrl: svgUrl(1000, 1600, `Page ${index + 1}`),
        mediaVersion: 1,
        pixelWidth: 1000,
        pixelHeight: 1600,
        finalBytes: 1000,
        error: null
      }));
      window.__NEWSPAPER_PERF__ = {
        commandCounts: {},
        get lastSavedPageId() {
          return lastSavedPageId;
        }
      };
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
          const counts = window.__NEWSPAPER_PERF__.commandCounts;
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
            case "get_newspaper_library_page": {
              const query = String(args.query ?? "").toLowerCase();
              const filtered = query
                ? items.filter((item) => `${item.editionName} ${item.editionCode} ${item.publicationDate}`.toLowerCase().includes(query))
                : items;
              const offset = Number(args.offset ?? 0);
              const limit = Number(args.limit ?? 50);
              return {
                items: filtered.slice(offset, offset + limit),
                total: filtered.length,
                offset,
                limit,
                revision: 1
              };
            }
            case "ensure_newspaper_thumbnail":
              return {
                status: "ready",
                thumbnailUrl: items.find((item) => item.jobId === args.jobId)?.thumbnailUrl,
                thumbnailVersion: "1-1",
                width: 420,
                height: 176
              };
            case "get_newspaper_reader_manifest":
              return pages.map((readerPage) => ({ ...readerPage, jobId: args.jobId }));
            case "save_newspaper_reading_progress": {
              const index = pages.findIndex((readerPage) => readerPage.id === args.pageId);
              lastSavedPageId = args.pageId;
              viewedPageIds.add(args.pageId);
              return {
                jobId: args.jobId,
                lastPageId: args.pageId,
                lastPageIndex: index,
                furthestPageIndex: Math.max(12, index),
                readPageCount: viewedPageIds.size,
                updatedAt: Date.now()
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
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener(_event, id) {
          callbacks.delete(id);
        }
      };
    }, { count: editionCount });

    const startedAt = performance.now();
    await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
    await page.getByRole("button", { name: "Newspaper library" }).click();
    await page.locator(".newspaper-library-row:not(.newspaper-library-row-skeleton)").first().waitFor();
    await page.getByRole("button", { name: "Open settings" }).click();
    const settingsDialog = page.getByRole("dialog", { name: "LinkedVault settings" });
    const settingsGeometry = await settingsDialog.evaluate((dialog) => {
      const grid = dialog.querySelector(".settings-grid");
      const controls = [...dialog.querySelectorAll("input, select, button")]
        .filter((control) => !control.getAttribute("aria-label")?.startsWith("Close LinkedVault"));
      return {
        dialogClientWidth: dialog.clientWidth,
        dialogScrollWidth: dialog.scrollWidth,
        gridClientWidth: grid?.clientWidth ?? 0,
        gridScrollWidth: grid?.scrollWidth ?? 1,
        tallestControl: Math.max(0, ...controls.map((control) => control.getBoundingClientRect().height))
      };
    });
    assert.ok(settingsGeometry.dialogScrollWidth <= settingsGeometry.dialogClientWidth + 1, "Settings dialog has horizontal overflow");
    assert.ok(settingsGeometry.gridScrollWidth <= settingsGeometry.gridClientWidth + 1, "Settings content exceeds its dialog");
    assert.ok(settingsGeometry.tallestControl <= 32.5, `Settings controls are not compact: ${settingsGeometry.tallestControl}px`);
    assert.equal(
      await page.locator(".lv-sidebar nav").evaluate((nav) => nav.scrollWidth <= nav.clientWidth + 1),
      true,
      "Sidebar navigation has horizontal overflow"
    );
    const defaultZoomControl = page.getByLabel("Default newspaper zoom");
    const clickZoomControl = page.getByLabel("Newspaper left-click zoom");
    const defaultToneControl = page.getByLabel("Default newspaper page tone");
    assert.equal(await defaultZoomControl.inputValue(), "100");
    assert.equal(await clickZoomControl.inputValue(), "120");
    assert.equal(await defaultToneControl.inputValue(), "soft");
    await page.getByRole("button", { name: "Close", exact: true }).click();
    assert.equal(
      await page.locator(".newspaper-library-toolbar").evaluate((toolbar) => toolbar.scrollWidth <= toolbar.clientWidth),
      true,
      "Library filters overflowed the toolbar"
    );
    const readyMs = performance.now() - startedAt;
    const mountedRows = await page.locator(".newspaper-library-row").count();
    assert.ok(mountedRows <= 16, `${editionCount}-edition Library mounted ${mountedRows} rows`);
    const mountedThumbnails = await page.locator(".newspaper-preview img").count();
    assert.ok(mountedThumbnails > 0, `${editionCount}-edition Library rendered no visible thumbnails`);
    assert.ok(
      mountedThumbnails <= 8,
      `${editionCount}-edition Library assigned image URLs to ${mountedThumbnails} rows`
    );
    const libraryCalls = await page.evaluate(
      () => window.__NEWSPAPER_PERF__.commandCounts.get_newspaper_library_page ?? 0
    );
    assert.ok(libraryCalls <= 2, `${editionCount}-edition initial view made ${libraryCalls} Library page calls`);

    const libraryCanvas = page.locator('[data-testid="newspaper-library-scroll"]');
    let savedLibraryScrollTop = 0;
    if (editionCount === 500) {
      savedLibraryScrollTop = await libraryCanvas.evaluate((element) => {
        element.scrollTop = Math.round((element.scrollHeight - element.clientHeight) * 0.72);
        return element.scrollTop;
      });
      await page.waitForTimeout(120);
      await page.locator(".newspaper-library-row:not(.newspaper-library-row-skeleton)").first().waitFor();
      assert.ok(savedLibraryScrollTop > 10_000, "Large Library fixture did not reach a deep scroll position");
      await page.getByRole("button", { name: "Open settings" }).click();
      await defaultToneControl.selectOption("dim");
      await defaultToneControl.selectOption("soft");
      await page.getByRole("button", { name: "Save settings" }).click();
      await page.getByRole("button", { name: "Close", exact: true }).click();
      assert.ok(
        Math.abs(await libraryCanvas.evaluate((element) => element.scrollTop) - savedLibraryScrollTop) <= 1,
        "Changing Reader preferences moved the deep Library scroll position"
      );
    }

    const libraryOpenButton = page.locator(".newspaper-library-open").nth(1);
    if (editionCount === 500) {
      await libraryOpenButton.evaluate((button) => button.click());
    } else {
      await libraryOpenButton.click();
    }
    await page.locator('[data-testid="newspaper-reader-page-image"]').first().waitFor();
    assert.equal(await page.locator(".newspaper-reader-zoom output").textContent(), "100%");
    assert.equal(await page.getByLabel("Newspaper page tone").inputValue(), "soft");
    const manifestCallsBeforePreferenceRerender = await page.evaluate(
      () => window.__NEWSPAPER_PERF__.commandCounts.get_newspaper_reader_manifest ?? 0
    );
    await page.evaluate(() => {
      window.dispatchEvent(new CustomEvent("linkvault:newspaper-reader-preferences", {
        detail: { defaultZoom: 1, clickZoom: 1.2, pageTone: "soft" }
      }));
    });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(
      await page.evaluate(() => window.__NEWSPAPER_PERF__.commandCounts.get_newspaper_reader_manifest ?? 0),
      manifestCallsBeforePreferenceRerender,
      "A parent-only Reader preference rerender reloaded the manifest"
    );
    assert.equal(
      await page.locator(".newspaper-reader-loading").count(),
      0,
      "A parent-only Reader preference rerender showed a loading flash"
    );
    await page.waitForFunction(() => {
      const canvas = document.querySelector('[data-testid="newspaper-reader-scroll"]');
      const image = document.querySelector('[data-testid="newspaper-reader-page-image"]');
      if (!canvas || !image) return false;
      const ratio = image.getBoundingClientRect().width / canvas.getBoundingClientRect().width;
      return ratio >= .96 && ratio <= 1.02;
    });
    await page.waitForFunction(() => {
      const canvasRect = document.querySelector('[data-testid="newspaper-reader-scroll"]')?.getBoundingClientRect();
      return canvasRect && [...document.querySelectorAll('[data-testid="newspaper-reader-page-image"]')]
        .some((image) => {
          const rect = image.getBoundingClientRect();
          return rect.bottom > canvasRect.top && rect.top < canvasRect.bottom;
        });
    });
    const renderedReader = await page.evaluate(() => {
      const canvas = document.querySelector('[data-testid="newspaper-reader-scroll"]');
      const canvasRect = canvas?.getBoundingClientRect();
      if (!canvas || !canvasRect) return null;
      const images = [...document.querySelectorAll('[data-testid="newspaper-reader-page-image"]')];
      const visibleImage = images
        .find((image) => {
          const rect = image.getBoundingClientRect();
          return rect.bottom > canvasRect.top && rect.top < canvasRect.bottom;
        });
      return {
        selectedIndex: visibleImage?.closest(".newspaper-reader-page")?.getAttribute("data-index") ?? null,
        activeIndex: document.querySelector('[aria-label="Select newspaper page"]')?.value ?? null,
        scrollTop: canvas.scrollTop,
        canvas: { top: canvasRect.top, bottom: canvasRect.bottom },
        images: images.map((image) => {
          const rect = image.getBoundingClientRect();
          return { top: rect.top, bottom: rect.bottom };
        })
      };
    });
    assert.ok(renderedReader?.selectedIndex, `Reader has no page image intersecting the viewport: ${JSON.stringify(renderedReader)}`);
    const selectedIndex = renderedReader.selectedIndex;
    const readerImage = page.locator(
      `.newspaper-reader-page[data-index="${selectedIndex}"] [data-testid="newspaper-reader-page-image"]`
    );
    await readerImage.waitFor();
    const readerCanvas = page.locator('[data-testid="newspaper-reader-scroll"]');
    const [imageBox, canvasBox, headerBox] = await Promise.all([
      readerImage.boundingBox(),
      readerCanvas.boundingBox(),
      page.locator(".newspaper-reader-header").boundingBox()
    ]);
    assert.ok(imageBox && canvasBox, "Reader image or canvas has no rendered bounds");
    assert.ok(
      imageBox.width / canvasBox.width >= .96 && imageBox.width / canvasBox.width <= 1.02,
      `Reader did not open near its 100% baseline: ${Math.round((imageBox.width / canvasBox.width) * 100)}%`
    );
    assert.ok(headerBox && headerBox.height <= 34, `Reader toolbar is ${headerBox?.height ?? 0}px tall`);
    assert.equal(
      await page.locator(".newspaper-reader-header").evaluate((header) => header.scrollWidth <= header.clientWidth),
      true,
      "Reader controls overflowed the compact toolbar"
    );
    assert.equal(
      await readerImage.evaluate((image) => getComputedStyle(image).cursor),
      "default",
      "The baseline Reader cursor must remain the arrow"
    );
    const baselineToneStyle = await readerImage.evaluate((image) => {
      const style = getComputedStyle(image);
      return { opacity: style.opacity, filter: style.filter };
    });
    assert.equal(baselineToneStyle.opacity, "1", "Soft paper must not allocate per-image opacity layers");
    assert.equal(baselineToneStyle.filter, "none", "Soft paper must not allocate an image filter layer");
    assert.notEqual(
      await page.locator(".newspaper-reader-tone-overlay").evaluate((overlay) => getComputedStyle(overlay).backgroundColor),
      "rgba(0, 0, 0, 0)",
      "Soft paper did not apply its single viewport tint"
    );
    const mountedLoadingModes = await page.locator('[data-testid="newspaper-reader-page-image"]').evaluateAll(
      (images) => images.map((image) => image.getAttribute("loading"))
    );
    assert.ok(
      mountedLoadingModes.every((loading) => loading === "eager"),
      `Reader left a bounded adjacent page lazy: ${mountedLoadingModes.join(", ")}`
    );
    const mountedPageRects = await page.locator(".newspaper-reader-page").evaluateAll(
      (articles) => articles
        .map((article) => article.getBoundingClientRect())
        .sort((left, right) => left.top - right.top)
        .map((rect) => ({ top: rect.top, bottom: rect.bottom }))
    );
    if (mountedPageRects.length > 1) {
      assert.ok(
        mountedPageRects[1].top - mountedPageRects[0].bottom <= 3,
        `Reader left a ${Math.round(mountedPageRects[1].top - mountedPageRects[0].bottom)}px dark page gap`
      );
    }
    const zoomClick = {
      x: imageBox.width * 0.72,
      y: Math.min(240, imageBox.height * 0.2)
    };
    const clickClient = {
      x: imageBox.x + zoomClick.x,
      y: imageBox.y + zoomClick.y
    };
    const clickRatio = {
      x: zoomClick.x / imageBox.width,
      y: zoomClick.y / imageBox.height
    };
    await page.mouse.click(clickClient.x, clickClient.y);
    await page.locator('[data-testid="newspaper-reader-page-image"][data-click-zoomed="true"]').first().waitFor();
    assert.equal(await page.locator(".newspaper-reader-zoom output").textContent(), "120%");
    await page.evaluate(() => new Promise((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(resolve))
    ));
    const zoomedImageBox = await readerImage.boundingBox();
    assert.ok(zoomedImageBox, "Zoomed reader image has no rendered bounds");
    assert.ok(
      Math.abs(zoomedImageBox.x + zoomedImageBox.width * clickRatio.x - clickClient.x) <= 3
        && Math.abs(zoomedImageBox.y + zoomedImageBox.height * clickRatio.y - clickClient.y) <= 3,
      "Click zoom did not keep the selected newspaper location under the pointer"
    );
    const imageTransitionProperties = await readerImage.evaluate(
      (image) => getComputedStyle(image).transitionProperty
    );
    assert.ok(
      !imageTransitionProperties.split(",").map((property) => property.trim()).includes("width"),
      "Reader still animates image width during click zoom"
    );
    assert.equal(
      await readerImage.evaluate((image) => getComputedStyle(image).cursor),
      "grab",
      "Click-zoomed Reader page did not expose the open drag hand"
    );
    const beforePan = await readerCanvas.evaluate((element) => ({
      left: element.scrollLeft,
      top: element.scrollTop
    }));
    await page.mouse.move(clickClient.x, clickClient.y);
    await page.mouse.down();
    await page.mouse.move(clickClient.x - 84, clickClient.y - 62, { steps: 6 });
    assert.equal(
      await page.locator(".newspaper-reader").getAttribute("data-panning"),
      "true",
      "Reader did not enter its active drag state"
    );
    assert.equal(
      await readerCanvas.evaluate((element) => getComputedStyle(element).cursor),
      "grabbing",
      "Active Reader drag did not expose the closed hand"
    );
    await page.mouse.up();
    const afterPan = await readerCanvas.evaluate((element) => ({
      left: element.scrollLeft,
      top: element.scrollTop
    }));
    assert.ok(afterPan.left >= beforePan.left + 70, "Reader drag did not pan horizontally");
    assert.ok(afterPan.top >= beforePan.top + 48, "Reader drag did not pan vertically");
    assert.equal(
      await page.locator(".newspaper-reader-zoom output").textContent(),
      "120%",
      "Reader drag incorrectly triggered the click zoom toggle"
    );
    const afterPanImageBox = await readerImage.boundingBox();
    assert.ok(afterPanImageBox, "Dragged Reader image has no rendered bounds");
    const zoomOutPosition = {
      x: Math.max(8, Math.min(afterPanImageBox.width - 8, clickClient.x - afterPanImageBox.x)),
      y: Math.max(8, Math.min(afterPanImageBox.height - 8, clickClient.y - afterPanImageBox.y))
    };
    await page.mouse.click(
      afterPanImageBox.x + zoomOutPosition.x,
      afterPanImageBox.y + zoomOutPosition.y
    );
    await page.locator('[data-testid="newspaper-reader-page-image"]:not([data-click-zoomed])').first().waitFor();
    assert.equal(await page.locator(".newspaper-reader-zoom output").textContent(), "100%");
    const restoredImageBox = await readerImage.boundingBox();
    assert.ok(
      restoredImageBox
        && restoredImageBox.y + restoredImageBox.height > canvasBox.y
        && restoredImageBox.y < canvasBox.y + canvasBox.height,
      "Click zoom returned to a blank reader viewport"
    );

    await page.waitForTimeout(120);
    await page.getByLabel("Select newspaper page").selectOption("17");
    await page.waitForFunction(() => {
      const select = document.querySelector('[aria-label="Select newspaper page"]');
      const selectedImage = document.querySelector(
        '.newspaper-reader-page[data-index="17"] [data-testid="newspaper-reader-page-image"]'
      );
      return select?.value === "17" && selectedImage;
    });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const beforeToneChange = await page.evaluate(() => {
      const canvas = document.querySelector('[data-testid="newspaper-reader-scroll"]');
      const select = document.querySelector('[aria-label="Select newspaper page"]');
      return {
        scrollTop: canvas?.scrollTop ?? -1,
        scrollHeight: canvas?.scrollHeight ?? -1,
        activeIndex: select?.value ?? null
      };
    });
    await page.getByLabel("Newspaper page tone").selectOption("dim");
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const afterToneChange = await page.evaluate(() => {
      const canvas = document.querySelector('[data-testid="newspaper-reader-scroll"]');
      const select = document.querySelector('[aria-label="Select newspaper page"]');
      const images = [...document.querySelectorAll('[data-testid="newspaper-reader-page-image"]')];
      return {
        scrollTop: canvas?.scrollTop ?? -1,
        activeIndex: select?.value ?? null,
        mountedImages: images.length,
        filters: images.map((image) => getComputedStyle(image).filter)
      };
    });
    assert.ok(
      Math.abs(afterToneChange.scrollTop - beforeToneChange.scrollTop) <= 1,
      `Changing page tone moved the settled deep Reader position from ${beforeToneChange.scrollTop} to ${afterToneChange.scrollTop}`
    );
    assert.equal(afterToneChange.activeIndex, beforeToneChange.activeIndex, "Changing page tone changed the active page");
    assert.ok(afterToneChange.mountedImages <= 3, "Changing page tone broke bounded Reader mounting");
    assert.ok(afterToneChange.filters.every((filter) => filter === "none"), "Dim paper allocated an image filter layer");
    assert.notEqual(
      await page.locator(".newspaper-reader-tone-overlay").evaluate((overlay) => getComputedStyle(overlay).backgroundColor),
      "rgba(0, 0, 0, 0)",
      "Dim paper did not apply its single viewport tint"
    );

    const measureScrollFrames = async (tone) => {
      await page.getByLabel("Newspaper page tone").selectOption(tone);
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      return readerCanvas.evaluate(async (element) => {
        const start = element.scrollTop;
        const gaps = [];
        let previous = performance.now();
        for (let index = 0; index < 24; index += 1) {
          await new Promise((resolve) => requestAnimationFrame(resolve));
          const now = performance.now();
          gaps.push(now - previous);
          previous = now;
          element.scrollTop = Math.min(element.scrollHeight - element.clientHeight, start + index * 18);
        }
        element.scrollTop = start;
        gaps.sort((left, right) => left - right);
        return gaps[Math.floor(gaps.length * .95)];
      });
    };
    const originalToneP95Ms = await measureScrollFrames("original");
    const softToneP95Ms = await measureScrollFrames("soft");
    const invertedToneP95Ms = await measureScrollFrames("inverted");
    assert.ok(
      softToneP95Ms <= Math.max(50, originalToneP95Ms * 2.5),
      `Soft paper regressed scroll frames from ${originalToneP95Ms.toFixed(1)}ms to ${softToneP95Ms.toFixed(1)}ms`
    );
    assert.ok(
      invertedToneP95Ms <= Math.max(50, originalToneP95Ms * 2.5),
      `Inverted paper regressed scroll frames from ${originalToneP95Ms.toFixed(1)}ms to ${invertedToneP95Ms.toFixed(1)}ms`
    );
    await page.getByLabel("Newspaper page tone").selectOption("soft");
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const postProfileViewport = await page.evaluate(() => {
      const canvas = document.querySelector('[data-testid="newspaper-reader-scroll"]');
      const canvasRect = canvas?.getBoundingClientRect();
      const images = [...document.querySelectorAll('[data-testid="newspaper-reader-page-image"]')];
      return {
        activeIndex: document.querySelector('[aria-label="Select newspaper page"]')?.value ?? null,
        scrollTop: canvas?.scrollTop ?? -1,
        canvas: canvasRect ? { top: canvasRect.top, bottom: canvasRect.bottom } : null,
        images: images.map((image) => {
          const rect = image.getBoundingClientRect();
          return {
            index: image.closest(".newspaper-reader-page")?.getAttribute("data-index") ?? null,
            top: rect.top,
            bottom: rect.bottom
          };
        })
      };
    });
    assert.ok(
      postProfileViewport.canvas && postProfileViewport.images.some((image) =>
        image.bottom > postProfileViewport.canvas.top && image.top < postProfileViewport.canvas.bottom
      ),
      `Reader lost the visible page after tone profiling: ${JSON.stringify(postProfileViewport)}`
    );
    if (editionCount === 8 && process.env.LINKVAULT_READER_SCREENSHOT) {
      await page.screenshot({ path: process.env.LINKVAULT_READER_SCREENSHOT });
    }
    await page.evaluate(() => {
      window.__NEWSPAPER_PERF__.maxMountedImages = document.querySelectorAll(
        '[data-testid="newspaper-reader-page-image"]'
      ).length;
      const observer = new MutationObserver(() => {
        window.__NEWSPAPER_PERF__.maxMountedImages = Math.max(
          window.__NEWSPAPER_PERF__.maxMountedImages,
          document.querySelectorAll('[data-testid="newspaper-reader-page-image"]').length
        );
      });
      observer.observe(document.body, { childList: true, subtree: true });
      window.__NEWSPAPER_PERF__.observer = observer;
    });
    for (const ratio of [0.25, 0.5, 0.9, 1]) {
      await readerCanvas.evaluate((element, nextRatio) => {
        element.scrollTop = (element.scrollHeight - element.clientHeight) * nextRatio;
      }, ratio);
      await page.waitForTimeout(80);
    }
    await page.waitForTimeout(450);
    const maxMountedImages = await page.evaluate(() => {
      window.__NEWSPAPER_PERF__.observer.disconnect();
      return window.__NEWSPAPER_PERF__.maxMountedImages;
    });
    assert.ok(maxMountedImages <= 3, `Reader mounted ${maxMountedImages} page images`);
    await page.getByRole("button", { name: "Back to library" }).click();
    await page.locator(".newspaper-library").waitFor();
    if (editionCount === 500) {
      await page.waitForFunction(
        (expected) => {
          const element = document.querySelector('[data-testid="newspaper-library-scroll"]');
          return element && Math.abs(element.scrollTop - expected) <= 2;
        },
        savedLibraryScrollTop
      );
    }
    const returnedProgress = page.locator(".newspaper-reading-progress").nth(1);
    assert.ok(
      Number(await returnedProgress.getAttribute("aria-valuenow")) < 100,
      "Fast-scrolling to the final page incorrectly marked every page viewed"
    );
    assert.match(
      await returnedProgress.getAttribute("title") ?? "",
      /\d+ of 38 pages viewed/,
      "Library progress does not explain unique viewed-page coverage"
    );
    const savedPageId = await page.evaluate(() => window.__NEWSPAPER_PERF__.lastSavedPageId);
    assert.ok(savedPageId, "Reader did not persist its active page before closing");

    if (editionCount === 8) {
      await page.getByRole("button", { name: "Open settings" }).click();
      await defaultZoomControl.selectOption("140");
      await defaultToneControl.selectOption("dim");
      await page.getByRole("button", { name: "Save settings" }).click();
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page.reload({ waitUntil: "domcontentloaded" });
      await page.getByRole("button", { name: "Newspaper library" }).click();
      await page.locator(".newspaper-library-row:not(.newspaper-library-row-skeleton)").first().waitFor();
      await page.getByRole("button", { name: "Open settings" }).click();
      assert.equal(await page.getByLabel("Default newspaper zoom").inputValue(), "140");
      assert.equal(await page.getByLabel("Default newspaper page tone").inputValue(), "dim");
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page.locator(".newspaper-library-open").first().click();
      await page.locator('[data-testid="newspaper-reader-page-image"]').first().waitFor();
      assert.equal(await page.locator(".newspaper-reader-zoom output").textContent(), "140%");
      assert.equal(await page.getByLabel("Newspaper page tone").inputValue(), "dim");
    }

    profiles.push({
      editionCount,
      readyMs: Math.round(readyMs),
      mountedRows,
      mountedThumbnails,
      libraryCalls,
      maxMountedImages,
      originalToneP95Ms: Number(originalToneP95Ms.toFixed(1)),
      softToneP95Ms: Number(softToneP95Ms.toFixed(1)),
      invertedToneP95Ms: Number(invertedToneP95Ms.toFixed(1))
    });
    await page.close();
  }
} finally {
  await browser.close();
}

console.table(profiles);
console.log("Browser-mocked newspaper performance profiles passed.");
