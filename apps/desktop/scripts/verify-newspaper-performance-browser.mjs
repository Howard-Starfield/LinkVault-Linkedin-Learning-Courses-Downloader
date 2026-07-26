import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a built LinkVault preview.");

const browser = await chromium.launch({
  channel: process.env.PLAYWRIGHT_CHANNEL || "chrome",
  headless: true
});

const profiles = [];
try {
  for (const editionCount of [8, 50, 500]) {
    const page = await browser.newPage({ viewport: { width: 1720, height: 960 } });
    await page.addInitScript(({ count }) => {
      const callbacks = new Map();
      let callbackId = 1;
      let lastSavedPageId = null;
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
              return {
                jobId: args.jobId,
                lastPageId: args.pageId,
                lastPageIndex: index,
                furthestPageIndex: Math.max(12, index),
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
    }, { count: editionCount });

    const startedAt = performance.now();
    await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
    await page.getByRole("button", { name: "Newspaper library" }).click();
    await page.locator(".newspaper-library-row:not(.newspaper-library-row-skeleton)").first().waitFor();
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

    await page.locator(".newspaper-library-open").first().click();
    await page.locator('[data-testid="newspaper-reader-page-image"]').first().waitFor();
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
    const readerScroll = page.locator('[data-testid="newspaper-reader-scroll"]');
    for (const ratio of [0.25, 0.5, 0.9, 0.35]) {
      await readerScroll.evaluate((element, nextRatio) => {
        element.scrollTop = (element.scrollHeight - element.clientHeight) * nextRatio;
      }, ratio);
      await page.waitForTimeout(80);
    }
    const maxMountedImages = await page.evaluate(() => {
      window.__NEWSPAPER_PERF__.observer.disconnect();
      return window.__NEWSPAPER_PERF__.maxMountedImages;
    });
    assert.ok(maxMountedImages <= 3, `Reader mounted ${maxMountedImages} page images`);
    await page.getByRole("button", { name: "Back to library" }).click();
    await page.locator(".newspaper-library").waitFor();
    const savedPageId = await page.evaluate(() => window.__NEWSPAPER_PERF__.lastSavedPageId);
    assert.ok(savedPageId, "Reader did not persist its active page before closing");

    profiles.push({
      editionCount,
      readyMs: Math.round(readyMs),
      mountedRows,
      mountedThumbnails,
      libraryCalls,
      maxMountedImages
    });
    await page.close();
  }
} finally {
  await browser.close();
}

console.table(profiles);
console.log("Browser-mocked newspaper performance profiles passed.");
