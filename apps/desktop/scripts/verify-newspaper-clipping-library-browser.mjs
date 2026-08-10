import assert from "node:assert/strict";
import { chromium } from "playwright";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a built LinkVault preview.");

const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1600, height: 960 } });
  await page.addInitScript(() => {
    const svg = (label) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="100%" height="100%" fill="#ede6d2"/><path d="M20 70h600M20 130h600M20 190h600M20 250h600" stroke="#8b806b"/><text x="24" y="42" fill="#27231d" font-size="22">${label}</text></svg>`)}`;
    const details = Array.from({ length: 500 }, (_, index) => ({
      id: `00000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
      title: `Transit archive clipping ${index + 1}`,
      noteMarkdown: index === 0 ? "Initial note about community transit" : `Research note ${index + 1}`,
      editionCode: `E${String(index % 13).padStart(2, "0")}`,
      editionName: `World Journal Edition ${index % 13 + 1}`,
      publicationDate: `2026-07-${String(index % 28 + 1).padStart(2, "0")}`,
      pageNumber: `A${index % 40 + 1}`,
      imageUrl: svg(`Clipping ${index + 1}`),
      sourceAvailable: true,
      assetState: "ready",
      assetErrorCode: null,
      storageStatus: "connected",
      assetWidth: 1200,
      assetHeight: 700,
      revision: 1,
      createdAt: 1000 + index,
      updatedAt: 2000 + index
    }));
    const summary = (detail) => ({
      id: detail.id,
      title: detail.title,
      noteExcerpt: detail.noteMarkdown.slice(0, 80),
      editionCode: detail.editionCode,
      editionName: detail.editionName,
      publicationDate: detail.publicationDate,
      pageNumber: detail.pageNumber,
      thumbnailReady: false,
      thumbnailUrl: null,
      thumbnailVersion: null,
      sourceAvailable: detail.sourceAvailable,
      assetState: detail.assetState,
      assetErrorCode: detail.assetErrorCode,
      assetWidth: detail.assetWidth,
      assetHeight: detail.assetHeight,
      revision: detail.revision,
      createdAt: detail.createdAt,
      updatedAt: detail.updatedAt
    });
    window.__CLIPPING_LIBRARY_TEST__ = {
      details,
      pageCalls: [],
      thumbnailCalls: [],
      updateCalls: [],
      searchCalls: [],
      rootChecks: 0,
      conflictNext: false,
      failNext: false
    };
    window.__NEWSPAPER_CLIPPINGS_API__ = {
      async getPage(request) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.pageCalls.push({ ...request, at: performance.now() });
        return {
          items: test.details.slice(request.offset, request.offset + request.limit).map(summary),
          total: test.details.length,
          offset: request.offset,
          limit: request.limit
        };
      },
      async getDetail(id) {
        return structuredClone(window.__CLIPPING_LIBRARY_TEST__.details.find((item) => item.id === id));
      },
      async update(request) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.updateCalls.push(structuredClone(request));
        const detail = test.details.find((item) => item.id === request.clippingId);
        await new Promise((resolve) => setTimeout(resolve, 30));
        if (test.failNext) {
          test.failNext = false;
          throw "CLIPPING_DATABASE_WRITE_FAILED";
        }
        if (test.conflictNext) {
          test.conflictNext = false;
          detail.noteMarkdown = "Saved in another window";
          detail.revision += 1;
          throw "CLIPPING_REVISION_CONFLICT";
        }
        if (request.expectedRevision !== detail.revision) throw "CLIPPING_REVISION_CONFLICT";
        detail.title = request.title.trim();
        detail.noteMarkdown = request.noteMarkdown;
        detail.revision += 1;
        detail.updatedAt += 1;
        return structuredClone(detail);
      },
      async ensureThumbnail(id) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.thumbnailCalls.push(id);
        return { status: "generated", thumbnailUrl: svg("Thumbnail"), thumbnailVersion: "1-1", width: 320, height: 180 };
      },
      async search(query, offset) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        const started = performance.now();
        const normalized = query.toLowerCase();
        let matches = test.details.filter((item) => `${item.title} ${item.noteMarkdown} ${item.editionName} ${item.editionCode} ${item.publicationDate} ${item.pageNumber}`.toLowerCase().includes(normalized));
        if (normalized === "transit") matches = test.details;
        test.searchCalls.push({ query, offset, elapsed: performance.now() - started });
        return {
          items: matches.slice(offset, offset + 50).map((detail) => ({
            clipping: summary(detail),
            matchedFields: detail.noteMarkdown.toLowerCase().includes(normalized) ? ["title", "note", "edition", "date", "page"] : ["title", "edition", "date", "page"],
            snippets: [{ field: "note", parts: [{ text: detail.noteMarkdown, highlighted: detail.noteMarkdown.toLowerCase().includes(normalized) }] }],
            possibleMatch: false
          })),
          total: matches.length,
          offset,
          limit: 50,
          noteSearchApplied: query.length >= 3,
          revision: 1
        };
      },
      async searchPossible(query) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        return {
          items: test.details.slice(475, 500).map((detail) => ({
            clipping: summary(detail), matchedFields: ["note"],
            snippets: [{ field: "note", parts: [{ text: `Possible ${query}`, highlighted: false }] }], possibleMatch: true
          })),
          limit: 25,
          revision: 1
        };
      },
      async listRoots() {
        return [{ rootId: "root-1", kind: "download_snapshot", displayPath: "D:\\Newspapers\\Newspaper snapshots", status: "offline", lastCheckedAt: null }];
      },
      async checkRoot(rootId) {
        window.__CLIPPING_LIBRARY_TEST__.rootChecks += 1;
        return { rootId, kind: "download_snapshot", displayPath: "D:\\Newspapers\\Newspaper snapshots", status: "connected", lastCheckedAt: 1 };
      },
      async reconnectRoot(rootId) {
        return { status: "connected", root: { rootId, kind: "download_snapshot", displayPath: "D:\\Newspapers\\Newspaper snapshots", status: "connected", lastCheckedAt: 1 } };
      },
      async openRoot() {}
    };
  });

  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    const location = message.location();
    if (location.url && new URL(location.url).pathname === "/favicon.ico") return;
    consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), false, "normal route fetched the editor chunk");

  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-detail").waitFor();
  await page.getByLabel("Clipping note editor body").waitFor();
  const mountedRows = await page.locator(".clipping-list__row").count();
  assert.ok(mountedRows > 0 && mountedRows <= 20, `virtual list mounted ${mountedRows} rows`);
  const thumbnailCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length);
  assert.ok(thumbnailCalls <= mountedRows, "thumbnail generation exceeded mounted rows");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), true, "detail did not lazy-load the editor chunk");

  const title = page.locator(".clipping-detail__title input");
  await title.fill("Transit evidence note");
  await page.getByLabel("Clipping note editor body").click();
  await page.keyboard.type(" with searchable keyword");
  await page.waitForTimeout(950);
  const initialSaveCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.updateCalls);
  assert.equal(initialSaveCalls.length, 2, "title blur plus stable editor draft did not produce two bounded saves");
  assert.equal(initialSaveCalls.at(-1).noteMarkdown.includes("searchable keyword"), true);
  await page.getByText("Saved", { exact: true }).waitFor();

  const globalSearch = page.getByLabel("Search saved newspaper clippings");
  await globalSearch.fill("transit");
  await page.locator(".clipping-search-results").waitFor();
  assert.ok(await page.getByText("Note", { exact: true }).count(), "search did not expose Note match tags");
  assert.ok(await page.getByText("Edition", { exact: true }).count(), "search did not expose Edition match tags");
  assert.equal(await page.locator(".clipping-possible-results .clipping-search-row").count(), 25);
  await page.locator(".clipping-search-more").scrollIntoViewIfNeeded();
  await page.waitForFunction(() => window.__CLIPPING_LIBRARY_TEST__.searchCalls.some((call) => call.offset === 50));
  const searchOffsets = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.searchCalls.map((call) => call.offset));
  assert.ok(searchOffsets.includes(50), "search scrolling did not lazy-load the next 50 results");
  await page.getByLabel("Clear clipping search").click();
  await page.locator(".clipping-detail").waitFor();

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.conflictNext = true; });
  await title.fill("Local conflict draft");
  await page.waitForTimeout(950);
  await page.getByText("This note changed in another window.").waitFor();
  assert.equal(await title.inputValue(), "Local conflict draft");
  await page.getByRole("button", { name: "Keep my changes" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.conflictNext = true; });
  await title.fill("Second local conflict draft");
  await page.waitForTimeout(950);
  await page.getByText("This note changed in another window.").waitFor();
  await page.getByRole("button", { name: "Keep my changes" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();

  await page.getByRole("button", { name: "Open settings" }).click();
  await page.getByText("D:\\Newspapers\\Newspaper snapshots").waitFor();
  await page.getByRole("button", { name: "Check again" }).click();
  await page.getByText("Connected", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.rootChecks), 1);
  await page.getByRole("button", { name: "Close", exact: true }).click();

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.failNext = true; });
  await title.fill("Draft that initially fails");
  await page.waitForTimeout(950);
  await page.getByText("Save failed. Your draft is still here.").waitFor();
  await page.getByRole("button", { name: "Download editions" }).click();
  assert.ok(await page.locator(".clipping-detail").count(), "failed flush allowed route navigation");
  await page.getByRole("button", { name: "Retry" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Download editions" }).click();
  await page.locator(".newspaper-download").waitFor();

  assert.deepEqual(consoleErrors, [], `browser console/page errors: ${consoleErrors.join("\n")}`);
  console.log("Clipping library browser matrix passed: virtualization, lazy editor, autosave, search paging/tags, conflict, roots, and guarded navigation.");
} finally {
  await browser.close();
}
