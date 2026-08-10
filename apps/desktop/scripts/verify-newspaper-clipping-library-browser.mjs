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
      assetVersion: 1,
      assetWidth: detail.assetWidth,
      assetHeight: detail.assetHeight,
      revision: detail.revision,
      createdAt: detail.createdAt,
      updatedAt: detail.updatedAt
    });
    window.__CLIPPING_LIBRARY_TEST__ = {
      details,
      pageCalls: [],
      detailCalls: [],
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
        window.__CLIPPING_LIBRARY_TEST__.detailCalls.push(id);
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
  assert.equal(await page.locator(".lv-global-search").count(), 0, "clipping search row leaked onto the default page");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), false, "normal route fetched the editor chunk");

  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery").waitFor();
  assert.equal(await page.locator(".clipping-gallery__header").count(), 0, "gallery still renders a second header below search");
  await page.getByText("Clippings", { exact: true }).last().waitFor();
  await page.getByText("500 clippings", { exact: true }).waitFor();
  assert.equal(await page.locator(".clipping-detail").count(), 0, "gallery eagerly mounted the clipping detail page");
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length), 0, "gallery eagerly fetched a clipping detail");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), false, "gallery eagerly fetched the editor chunk");
  const firstGridRow = page.locator(".clipping-gallery__row").first();
  await firstGridRow.waitFor();
  assert.equal(await firstGridRow.locator(".clipping-gallery__card").count(), 4, "default window did not render four clipping columns");
  const mountedCards = await page.locator(".clipping-gallery__card").count();
  assert.ok(mountedCards > 0 && mountedCards <= 40, `virtual gallery mounted ${mountedCards} cards`);
  const thumbnailCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length);
  assert.ok(thumbnailCalls > 0 && thumbnailCalls <= 80, `thumbnail generation was not viewport-bounded: ${thumbnailCalls} calls`);
  const firstCard = page.locator(".clipping-gallery__card").first();
  await firstCard.locator(".clipping-gallery__thumb img[data-loaded='true']").waitFor();
  const cardGeometry = await firstCard.evaluate((card) => {
    const thumbnail = card.querySelector(".clipping-gallery__thumb").getBoundingClientRect();
    const title = card.querySelector(".clipping-gallery__title").getBoundingClientRect();
    const bounds = card.getBoundingClientRect();
    return {
      cardWidth: bounds.width,
      thumbnailWidth: thumbnail.width,
      thumbnailHeight: thumbnail.height,
      titleLeft: title.left,
      titleBottom: title.bottom,
      titleHeight: title.height,
      thumbnailLeft: thumbnail.left,
      thumbnailBottom: thumbnail.bottom
    };
  });
  assert.ok(cardGeometry.thumbnailWidth >= cardGeometry.cardWidth - 2, "thumbnail does not occupy the full gallery card width");
  assert.ok(Math.abs(cardGeometry.thumbnailWidth / cardGeometry.thumbnailHeight - 1200 / 700) < 0.03, "gallery card ignored the clipping aspect ratio");
  assert.ok(Math.abs(cardGeometry.titleLeft - cardGeometry.thumbnailLeft) <= 1, "title is not anchored to the thumbnail's bottom-left veil");
  assert.ok(cardGeometry.titleHeight < 60, "title gradient covers too much of the clipping");
  assert.ok(Math.abs(cardGeometry.thumbnailBottom - cardGeometry.titleBottom) <= 1, "title gradient bleeds beyond the thumbnail bottom edge");
  assert.equal((await firstCard.innerText()).trim(), "Transit archive clipping 1", "gallery card must show only its single title");
  await firstCard.hover();
  await page.waitForTimeout(220);
  const hoverTransforms = await firstCard.evaluate((card) => {
    const image = new DOMMatrixReadOnly(getComputedStyle(card.querySelector(".clipping-gallery__thumb img")).transform);
    const thumbnail = new DOMMatrixReadOnly(getComputedStyle(card.querySelector(".clipping-gallery__thumb")).transform);
    return { imageScaleX: image.a, imageScaleY: image.d, thumbnailX: thumbnail.e, thumbnailY: thumbnail.f };
  });
  assert.ok(Math.abs(hoverTransforms.imageScaleX - 1.05) < 0.01 && Math.abs(hoverTransforms.imageScaleY - 1.05) < 0.01, "hover does not enlarge the clipping image by 5%");
  assert.ok(Math.abs(hoverTransforms.thumbnailX) < 0.01 && Math.abs(hoverTransforms.thumbnailY) < 0.01, "hover still moves the clipping card");
  const callsAtDefaultWidth = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length);
  await page.setViewportSize({ width: 700, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 2);
  assert.equal(await page.locator(".clipping-gallery__row").first().locator(".clipping-gallery__card").count(), 2, "narrow gallery did not reduce its column count");
  await page.setViewportSize({ width: 1900, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 5);
  assert.equal(await page.locator(".clipping-gallery__row").first().locator(".clipping-gallery__card").count(), 5, "wide gallery did not add a clipping column");
  await page.waitForFunction((previous) => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length > previous, callsAtDefaultWidth);
  await page.setViewportSize({ width: 1600, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 4);
  if (process.env.LINKVAULT_CLIPPING_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_SCREENSHOT });
  }
  await firstCard.click();
  await page.locator(".clipping-detail").waitFor();
  await page.getByLabel("Clipping note editor body").waitFor();
  const topBack = page.locator(".lv-global-search").getByRole("button", { name: "Back", exact: true });
  await topBack.waitFor();
  assert.equal(await page.locator(".clipping-note-page__header").count(), 0, "detail still renders a redundant internal header");
  assert.equal(await page.getByLabel("Search saved newspaper clippings").count(), 0, "gallery search input remained mounted on the note detail page");
  assert.equal(await page.locator(".clipping-note-editor__utility-bar").count(), 0, "history controls still interrupt the title-to-editor flow");
  assert.equal(await page.locator(".clipping-note-editor__footer .clipping-save-status").count(), 1, "save state is not inside the note footer");
  assert.equal(await page.locator(".clipping-note-editor__footer").getByRole("toolbar", { name: "Editing history", exact: true }).count(), 1, "Undo and Redo are not inside the note footer");
  assert.equal(await page.locator(".lv-global-search__title-slot .clipping-detail__title input").count(), 1, "editable note title is not beside Back in the top bar");
  assert.equal(await page.locator(".clipping-detail__writing > .clipping-detail__title").count(), 0, "note title is still duplicated above the editor body");
  if (process.env.LINKVAULT_CLIPPING_DETAIL_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_DETAIL_SCREENSHOT });
  }
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length), 1, "thumbnail selection did not fetch exactly one clipping detail");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), true, "detail did not lazy-load the editor chunk");
  await topBack.click();
  await page.locator(".clipping-gallery").waitFor();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByLabel("Clipping note editor body").waitFor();

  const title = page.locator(".clipping-detail__title input");
  await title.fill("Transit evidence note");
  await title.press("Enter");
  assert.equal(await page.getByLabel("Clipping note editor body").evaluate((element) => element === document.activeElement), true, "Enter in the top-bar title did not focus the note body");
  await page.keyboard.type(" with searchable keyword");
  await page.waitForTimeout(950);
  const initialSaveCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.updateCalls);
  assert.equal(initialSaveCalls.length, 2, "title blur plus stable editor draft did not produce two bounded saves");
  assert.equal(initialSaveCalls.at(-1).noteMarkdown.includes("searchable keyword"), true);
  await page.getByText("Saved", { exact: true }).waitFor();

  await topBack.click();
  await page.locator(".clipping-gallery").waitFor();
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
  await page.locator(".clipping-gallery").waitFor();
  assert.equal(await page.locator(".clipping-detail").count(), 0, "clearing search bypassed the clipping gallery");
  await page.locator(".clipping-gallery__card").first().click();
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

  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery").waitFor();
  assert.equal(await page.locator(".clipping-detail").count(), 0, "Clippings navigation did not return to the gallery");
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByLabel("Clipping note editor body").waitFor();

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
  assert.equal(await page.locator(".lv-global-search").count(), 0, "clipping search row remained on Download editions");

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.details = []; });
  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.getByRole("heading", { name: "No clippings yet" }).waitFor();
  assert.equal(await page.locator(".clipping-gallery__skeletons:not(.is-loading) > span").count(), 4, "empty gallery does not show four clipping skeletons");
  await page.getByText("Clips you save from Newspaper library appear here with their notes.", { exact: true }).waitFor();
  await page.getByText("0 clippings", { exact: true }).waitFor();
  if (process.env.LINKVAULT_CLIPPING_EMPTY_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_EMPTY_SCREENSHOT });
  }
  await page.getByRole("button", { name: "Open Newspaper library" }).click();
  await page.locator(".newspaper-library").waitFor();

  assert.deepEqual(consoleErrors, [], `browser console/page errors: ${consoleErrors.join("\n")}`);
  console.log("Clipping library browser matrix passed: compact search-row summary, responsive gallery, first-use skeletons, contained title veil, lazy thumbnails/detail/editor, autosave, search, conflict, roots, and guarded navigation.");
} finally {
  await browser.close();
}
