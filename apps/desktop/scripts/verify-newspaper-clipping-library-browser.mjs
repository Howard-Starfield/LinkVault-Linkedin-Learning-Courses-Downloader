import assert from "node:assert/strict";
import { chromium } from "playwright";
import { installClippingNoteExitBrowserHarness } from "./clipping-note-exit-browser-harness.mjs";
import { installClippingInstanceActivationBrowserHarness } from "./clipping-instance-activation-browser-harness.mjs";
import { measureClippingNoteDurabilityBrowser } from "./clipping-note-durability-browser-performance.mjs";

const previewUrl = process.env.LINKVAULT_PREVIEW_URL;
assert.ok(previewUrl, "Set LINKVAULT_PREVIEW_URL to a built LinkVault preview.");

const browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chrome", headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1600, height: 960 } });
  await page.addInitScript(() => {
    const svg = (label) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="700"><rect width="100%" height="100%" fill="#ede6d2"/><path d="M20 70h1160M20 190h1160M20 310h1160M20 430h1160M20 550h1160M20 680h1160" stroke="#8b806b"/><text x="24" y="42" fill="#27231d" font-size="22">${label}</text></svg>`)}`;
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
      sourceJobId: `source-job-${index + 1}`,
      sourcePageId: `source-page-${index + 1}`,
      sourceMediaVersionSnapshot: 7,
      normalizedRect: { x: 0.2, y: 0.62, width: 0.35, height: 0.18 },
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
      deleteCalls: [],
      recoveryCalls: [],
      progressCalls: [],
      checkpointCalls: [],
      checkpointFail: false,
      recoveryDrafts: new Map(),
      invalidRecoveryIds: new Set(),
      searchCalls: [],
      rootChecks: 0,
      conflictNext: false,
      failNext: false,
      updateDelayMs: 30
    };
    window.__NEWSPAPER_CLIPPINGS_API__ = {
      async getLibraryItem(jobId) {
        const detail = window.__CLIPPING_LIBRARY_TEST__.details.find((item) => item.sourceJobId === jobId);
        if (!detail) throw "NEWSPAPER_SOURCE_JOB_NOT_FOUND";
        return {
          jobId,
          editionCode: detail.editionCode,
          editionName: detail.editionName,
          publicationDate: detail.publicationDate,
          status: "completed",
          outputDir: "D:\\Newspapers\\edition",
          pageCount: 3,
          completedCount: 3,
          updatedAt: detail.updatedAt,
          thumbnailReady: false,
          readPageCount: 0
        };
      },
      async getReaderManifest(jobId) {
        const detail = window.__CLIPPING_LIBRARY_TEST__.details.find((item) => item.sourceJobId === jobId);
        if (!detail) throw "NEWSPAPER_SOURCE_JOB_NOT_FOUND";
        return [
          { id: `${detail.sourcePageId}-other-1`, jobId, canonicalIndex: 0, pageNumber: "A01", status: "completed", mediaUrl: svg("Other page 1"), mediaVersion: 7, pixelWidth: 1200, pixelHeight: 700 },
          { id: `${detail.sourcePageId}-other-2`, jobId, canonicalIndex: 1, pageNumber: "A02", status: "completed", mediaUrl: svg("Other page 2"), mediaVersion: 7, pixelWidth: 1200, pixelHeight: 700 },
          { id: detail.sourcePageId, jobId, canonicalIndex: 2, pageNumber: detail.pageNumber, status: "completed", mediaUrl: svg("Exact source page"), mediaVersion: 7, pixelWidth: 1200, pixelHeight: 700 }
        ];
      },
      async saveReadingProgress(jobId, pageId) {
        window.__CLIPPING_LIBRARY_TEST__.progressCalls.push({ jobId, pageId });
        return { jobId, lastPageId: pageId, lastPageIndex: 2, furthestPageIndex: 2, readPageCount: 1, updatedAt: Date.now() };
      },
      async checkpoint(request) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.checkpointCalls.push(structuredClone(request));
        if (test.checkpointFail) throw "CLIPPING_RECOVERY_FAILED";
        const existing = test.recoveryDrafts.get(request.clippingId);
        if (existing && existing.writerSessionId !== request.writerSessionId) throw "CLIPPING_RECOVERY_WRITER_CONFLICT";
        if (existing && existing.writerSequence > request.writerSequence) throw "CLIPPING_RECOVERY_STALE_SEQUENCE";
        test.recoveryDrafts.set(request.clippingId, { ...structuredClone(request), updatedAt: Date.now() });
        return {
          clippingId: request.clippingId,
          writerSessionId: request.writerSessionId,
          writerSequence: request.writerSequence
        };
      },
      async loadRecovery(clippingId) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        const detail = test.details.find((item) => item.id === clippingId);
        const draft = test.recoveryDrafts.get(clippingId);
        if (!draft) return { status: "none", canonicalRevision: detail.revision, identity: null, draft: null };
        if (test.invalidRecoveryIds.has(clippingId)) {
          return {
            status: "invalid",
            canonicalRevision: detail.revision,
            identity: { clippingId, writerSessionId: draft.writerSessionId, writerSequence: draft.writerSequence },
            draft: null
          };
        }
        return {
          status: draft.baseRevision === detail.revision ? "matching" : "canonical_changed",
          canonicalRevision: detail.revision,
          identity: {
            clippingId,
            writerSessionId: draft.writerSessionId,
            writerSequence: draft.writerSequence
          },
          draft: {
            baseRevision: draft.baseRevision,
            title: draft.title,
            markdown: draft.markdown,
            updatedAt: draft.updatedAt
          }
        };
      },
      async claimRecovery(request) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        const draft = test.recoveryDrafts.get(request.clippingId);
        if (!draft || draft.writerSessionId !== request.priorWriterSessionId || draft.writerSequence !== request.priorWriterSequence) {
          throw "CLIPPING_RECOVERY_WRITER_CONFLICT";
        }
        draft.writerSessionId = request.writerSessionId;
        draft.writerSequence = 1;
        return window.__NEWSPAPER_CLIPPINGS_API__.loadRecovery(request.clippingId);
      },
      async discardRecovery(identity) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        const draft = test.recoveryDrafts.get(identity.clippingId);
        if (!draft) return;
        if (draft.writerSessionId !== identity.writerSessionId) throw "CLIPPING_RECOVERY_WRITER_CONFLICT";
        if (draft.writerSequence !== identity.writerSequence) throw "CLIPPING_RECOVERY_STALE_SEQUENCE";
        test.recoveryDrafts.delete(identity.clippingId);
        test.invalidRecoveryIds.delete(identity.clippingId);
      },
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
        await new Promise((resolve) => setTimeout(resolve, test.updateDelayMs));
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
        const draft = test.recoveryDrafts.get(request.clippingId);
        if (request.checkpoint && draft
          && draft.writerSessionId === request.checkpoint.writerSessionId
          && draft.writerSequence <= request.checkpoint.writerSequence) {
          test.recoveryDrafts.delete(request.clippingId);
        }
        return structuredClone(detail);
      },
      async delete(request) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.deleteCalls.push(structuredClone(request));
        const index = test.details.findIndex((item) => item.id === request.clippingId);
        if (index < 0) throw "CLIPPING_NOT_FOUND";
        if (test.details[index].revision !== request.expectedRevision) throw "CLIPPING_REVISION_CONFLICT";
        test.details.splice(index, 1);
        return { clippingId: request.clippingId, deleted: true };
      },
      async recoverAsset(clippingId) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.recoveryCalls.push(clippingId);
        const detail = test.details.find((item) => item.id === clippingId);
        if (!detail) throw "CLIPPING_NOT_FOUND";
        detail.assetState = "ready";
        detail.assetErrorCode = null;
        return structuredClone(detail);
      },
      async ensureThumbnail(id) {
        const test = window.__CLIPPING_LIBRARY_TEST__;
        test.thumbnailCalls.push(id);
        return { status: "generated", thumbnailUrl: svg("Thumbnail"), thumbnailVersion: "1-1", width: 1024, height: 597 };
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
  await page.addInitScript(installClippingNoteExitBrowserHarness);
  await page.addInitScript(installClippingInstanceActivationBrowserHarness);

  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    const location = message.location();
    if (location.url && new URL(location.url).pathname === "/favicon.ico") return;
    consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  await page.goto(previewUrl, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.__CLIPPING_NOTE_EXIT_BRIDGE__.listenerCounts().prepare === 1);
  assert.deepEqual(await page.evaluate(() => window.__CLIPPING_NOTE_EXIT_BRIDGE__.listenerCounts()), { prepare: 1, blocked: 1 }, "Strict Mode left duplicate native lifecycle listeners");
  assert.equal(await page.locator(".lv-global-search").count(), 0, "clipping search row leaked onto the default page");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), false, "normal route fetched the editor chunk");

  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery").waitFor();
  await page.waitForFunction(() => window.__CLIPPING_INSTANCE_ACTIVATION__.listenerCount() === 1);
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
    const image = card.querySelector(".clipping-gallery__thumb img");
    const imageBounds = image.getBoundingClientRect();
    const bounds = card.getBoundingClientRect();
    return {
      cardWidth: bounds.width,
      thumbnailWidth: thumbnail.width,
      thumbnailHeight: thumbnail.height,
      thumbnailTop: thumbnail.top,
      titleLeft: title.left,
      titleTop: title.top,
      thumbnailLeft: thumbnail.left,
      thumbnailBottom: thumbnail.bottom,
      imageLeft: imageBounds.left,
      imageTop: imageBounds.top,
      imageRight: imageBounds.right,
      imageBottom: imageBounds.bottom,
      imageNaturalRatio: image.naturalWidth / image.naturalHeight,
      objectFit: getComputedStyle(image).objectFit
    };
  });
  assert.ok(cardGeometry.thumbnailWidth >= cardGeometry.cardWidth - 2, "thumbnail does not occupy the full gallery card width");
  assert.ok(Math.abs(cardGeometry.thumbnailWidth / cardGeometry.thumbnailHeight - 1200 / 700) < 0.03, "gallery card ignored the clipping aspect ratio");
  assert.ok(Math.abs(cardGeometry.imageNaturalRatio - 1200 / 700) < 0.001, "browser fixture no longer represents the clipping's full aspect ratio");
  assert.equal(cardGeometry.objectFit, "contain", "gallery image can crop the canonical clipping");
  assert.ok(cardGeometry.imageLeft >= cardGeometry.thumbnailLeft - 1 && cardGeometry.imageTop >= cardGeometry.thumbnailTop - 1, "gallery image escapes its full-size frame");
  assert.ok(cardGeometry.imageRight <= cardGeometry.thumbnailLeft + cardGeometry.thumbnailWidth + 1 && cardGeometry.imageBottom <= cardGeometry.thumbnailBottom + 1, "gallery image is clipped outside its frame");
  assert.ok(Math.abs(cardGeometry.titleLeft - cardGeometry.thumbnailLeft) <= 3, "title is not anchored below the thumbnail's left edge");
  assert.ok(cardGeometry.titleTop >= cardGeometry.thumbnailBottom, "title still covers clipping pixels");
  assert.equal((await firstCard.innerText()).trim(), "Transit archive clipping 1", "gallery card must show only its single title");
  await firstCard.hover();
  await page.waitForTimeout(220);
  const hoverTransforms = await firstCard.evaluate((card) => {
    const image = new DOMMatrixReadOnly(getComputedStyle(card.querySelector(".clipping-gallery__thumb img")).transform);
    const thumbnail = new DOMMatrixReadOnly(getComputedStyle(card.querySelector(".clipping-gallery__thumb")).transform);
    return { imageScaleX: image.a, imageScaleY: image.d, thumbnailScaleX: thumbnail.a, thumbnailScaleY: thumbnail.d, thumbnailX: thumbnail.e, thumbnailY: thumbnail.f };
  });
  assert.ok(Math.abs(hoverTransforms.imageScaleX - 1) < 0.01 && Math.abs(hoverTransforms.imageScaleY - 1) < 0.01, "hover still crops by enlarging the image inside its frame");
  assert.ok(Math.abs(hoverTransforms.thumbnailScaleX - 1.05) < 0.01 && Math.abs(hoverTransforms.thumbnailScaleY - 1.05) < 0.01, "hover does not enlarge the complete clipping thumbnail by 5%");
  assert.ok(Math.abs(hoverTransforms.thumbnailX) < 0.01 && Math.abs(hoverTransforms.thumbnailY) < 0.01, "hover still moves the clipping card");
  const callsAtDefaultWidth = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length);
  await page.setViewportSize({ width: 700, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 2);
  assert.equal(await page.locator(".clipping-gallery__row").first().locator(".clipping-gallery__card").count(), 2, "narrow gallery did not reduce its column count");
  await page.setViewportSize({ width: 1900, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 5);
  assert.equal(await page.locator(".clipping-gallery__row").first().locator(".clipping-gallery__card").count(), 5, "wide gallery did not add a clipping column");
  const callsAtWideWidth = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.thumbnailCalls.length);
  assert.ok(callsAtWideWidth >= callsAtDefaultWidth && callsAtWideWidth <= 80, `responsive resize escaped viewport-bounded thumbnails: ${callsAtWideWidth} calls`);
  await page.setViewportSize({ width: 1600, height: 960 });
  await page.waitForFunction(() => document.querySelector(".clipping-gallery__row")?.children.length === 4);
  if (process.env.LINKVAULT_CLIPPING_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_SCREENSHOT });
  }
  await firstCard.click();
  await page.locator(".clipping-detail").waitFor();
  await page.getByLabel("Clipping note editor body").waitFor();
  assert.equal(await page.getByLabel("Clipping note editor body").evaluate((element) => element === document.activeElement), false, "ordinary gallery selection stole editor focus");
  const topBack = page.locator(".lv-global-search").getByRole("button", { name: "Back", exact: true });
  await topBack.waitFor();
  assert.equal(await page.locator(".clipping-note-page__header").count(), 0, "detail still renders a redundant internal header");
  assert.equal(await page.getByLabel("Search saved newspaper clippings").count(), 0, "gallery search input remained mounted on the note detail page");
  assert.equal(await page.locator(".clipping-note-editor__utility-bar").count(), 0, "history controls still interrupt the title-to-editor flow");
  assert.equal(await page.locator(".clipping-note-editor__footer .clipping-save-status").count(), 1, "save state is not inside the note footer");
  assert.equal(await page.locator(".clipping-note-editor__footer").getByRole("toolbar", { name: "Editing history", exact: true }).count(), 1, "Undo and Redo are not inside the note footer");
  assert.equal(await page.locator(".lv-global-search__title-slot .clipping-detail__title input").count(), 1, "editable note title is not beside Back in the top bar");
  assert.equal(await page.locator(".clipping-detail__writing > .clipping-detail__title").count(), 0, "note title is still duplicated above the editor body");
  const integratedClipping = page.locator(".clipping-note-editor > .clipping-source-card");
  await integratedClipping.waitFor();
  assert.equal(await page.locator(".clipping-note-editor__content img").count(), 0, "canonical clipping leaked into Markdown-owned editor content");
  await page.setViewportSize({ width: 640, height: 900 });
  assert.equal(await page.getByRole("toolbar", { name: "Clipping image alignment" }).isVisible(), false, "narrow editor exposed controls whose float layout is intentionally disabled");
  assert.equal(await integratedClipping.evaluate((element) => getComputedStyle(element).float), "none", "narrow editor did not restore full-width clipping flow");
  await page.setViewportSize({ width: 1600, height: 960 });
  const clippingMedia = integratedClipping.locator(".clipping-source-card__media");
  const clippingCaption = clippingMedia.locator("figcaption");
  await page.mouse.move(2, 2);
  await page.waitForTimeout(170);
  assert.equal(await clippingCaption.evaluate((element) => Number(getComputedStyle(element).opacity) < 0.05), true, "clipping title chrome is visible before hover");
  const compactMediaGeometry = await integratedClipping.evaluate((element) => {
    const card = element.getBoundingClientRect();
    const media = element.querySelector(".clipping-source-card__media").getBoundingClientRect();
    const caption = element.querySelector("figcaption");
    return {
      cardBottom: card.bottom,
      mediaBottom: media.bottom,
      captionPosition: getComputedStyle(caption).position
    };
  });
  assert.equal(compactMediaGeometry.captionPosition, "absolute", "clipping title still reserves float height");
  assert.ok(Math.abs(compactMediaGeometry.cardBottom - compactMediaGeometry.mediaBottom) < 1, "clipping chrome leaves dead space below the media rectangle");
  await clippingMedia.hover();
  await page.waitForFunction(() => Number(getComputedStyle(document.querySelector(".clipping-source-card__media > figcaption")).opacity) > .95);
  assert.equal(await clippingCaption.evaluate((element) => getComputedStyle(element).pointerEvents), "auto", "hovered title and source actions are not interactive");
  assert.equal(await page.getByRole("toolbar", { name: "Clipping image alignment" }).evaluate((element) => getComputedStyle(element).pointerEvents), "auto", "hovered alignment controls are not interactive");
  await page.getByRole("button", { name: "Align clipping left" }).click();
  assert.equal(await integratedClipping.getAttribute("data-alignment"), "left", "left newspaper alignment did not apply");
  assert.equal(await integratedClipping.evaluate((element) => getComputedStyle(element).float), "left", "clipping did not float beside note text");
  const wrapGeometry = await page.locator(".clipping-note-editor").evaluate((root) => {
    const clipping = root.querySelector(".clipping-source-card").getBoundingClientRect();
    const content = root.querySelector(".clipping-note-editor__content");
    const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
    let text = walker.nextNode();
    while (text && !text.textContent.trim()) text = walker.nextNode();
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 1);
    const firstCharacter = range.getBoundingClientRect();
    return { clippingRight: clipping.right, clippingBottom: clipping.bottom, textLeft: firstCharacter.left, textTop: firstCharacter.top };
  });
  assert.ok(
    wrapGeometry.textLeft > wrapGeometry.clippingRight && wrapGeometry.textTop < wrapGeometry.clippingBottom,
    `note text did not start beside the floated clipping: ${JSON.stringify(wrapGeometry)}`
  );
  await page.getByRole("button", { name: "Align clipping right" }).click();
  assert.equal(await integratedClipping.getAttribute("data-alignment"), "right", "right newspaper alignment did not apply");
  assert.equal(await integratedClipping.evaluate((element) => getComputedStyle(element).float), "right", "right clipping alignment did not change layout");
  assert.equal(await page.evaluate(() => localStorage.getItem("linkvault.clippingImageAlignment.v1")), "right", "presentation alignment was not retained locally");
  await page.getByRole("button", { name: "Zoom clipping image" }).click();
  const preview = page.getByRole("dialog", { name: "Clipping image preview" });
  await preview.waitFor();
  assert.equal(await preview.evaluate((element) => element.open), true, "left click did not open the canonical clipping preview");
  assert.equal(
    await preview.locator("img").getAttribute("src"),
    await page.getByRole("button", { name: "Zoom clipping image" }).locator("img").getAttribute("src"),
    "zoom preview did not reuse the canonical clipping URL"
  );
  const previewImage = preview.getByRole("button", { name: "Zoom in clipping preview" });
  await previewImage.waitFor();
  const previewHitLayer = await previewImage.evaluate((image) => {
    const rect = image.getBoundingClientRect();
    const target = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return {
      hitActualImage: target === image,
      imagePointerEvents: getComputedStyle(image).pointerEvents,
      viewportPointerEvents: getComputedStyle(image.parentElement).pointerEvents
    };
  });
  assert.deepEqual(previewHitLayer, { hitActualImage: true, imagePointerEvents: "auto", viewportPointerEvents: "none" }, "preview layers intercept the cropped image hit target");
  await previewImage.click();
  await page.waitForFunction(() => document.querySelector("dialog[aria-label='Clipping image preview']")?.dataset.zoomed === "true");
  const pannedImage = preview.getByRole("button", { name: "Fit clipping preview" });
  const panBefore = await pannedImage.evaluate((image) => ({
    x: image.style.getPropertyValue("--clipping-preview-x"),
    y: image.style.getPropertyValue("--clipping-preview-y")
  }));
  const panBox = await pannedImage.boundingBox();
  assert.ok(panBox, "zoomed clipping image has no hit-testable bounds");
  await page.mouse.move(panBox.x + panBox.width / 2, panBox.y + panBox.height / 2);
  await page.mouse.down({ button: "left" });
  await page.mouse.move(panBox.x + panBox.width / 2 + 70, panBox.y + panBox.height / 2 + 35, { steps: 5 });
  await page.mouse.up({ button: "left" });
  const panAfter = await pannedImage.evaluate((image) => ({
    x: image.style.getPropertyValue("--clipping-preview-x"),
    y: image.style.getPropertyValue("--clipping-preview-y"),
    dragging: image.dataset.dragging
  }));
  assert.notDeepEqual({ x: panAfter.x, y: panAfter.y }, panBefore, "left-button drag did not pan the magnified clipping image");
  assert.equal(panAfter.dragging, "false", "preview retained its dragging layer state after pointer release");
  assert.equal(await preview.getAttribute("data-zoomed"), "true", "drag gesture was misread as a click-to-fit action");
  await pannedImage.click();
  await page.waitForFunction(() => document.querySelector("dialog[aria-label='Clipping image preview']")?.dataset.zoomed === "false");
  assert.equal(await preview.getByRole("button", { name: "Zoom in clipping preview" }).count(), 1, "second left click did not restore fit mode");
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("dialog[aria-label='Clipping image preview']")?.open);
  assert.equal(await page.getByRole("button", { name: "Zoom clipping image" }).evaluate((element) => element === document.activeElement), true, "closing image preview did not restore trigger focus");
  await page.locator(".clipping-source-card__image-button > img").evaluate((image) => image.dispatchEvent(new Event("error")));
  await page.getByRole("button", { name: "Retry image check" }).click();
  await page.locator(".clipping-source-card__image-button > img").waitFor();
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.recoveryCalls.length), 1, "successful exact-asset retry did not remount the verified clipping image");
  if (process.env.LINKVAULT_CLIPPING_DETAIL_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_DETAIL_SCREENSHOT });
  }
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length), 1, "thumbnail selection did not fetch exactly one clipping detail");
  assert.equal((await page.evaluate(() => performance.getEntriesByType("resource").some((entry) => entry.name.includes("ClippingNoteEditor")))), true, "detail did not lazy-load the editor chunk");
  await page.waitForFunction(() => window.__CLIPPING_INSTANCE_ACTIVATION__.listenerCount() === 2);
  const callsBeforeActivation = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length);
  await page.evaluate(async () => {
    const detail = window.__CLIPPING_LIBRARY_TEST__.details[0];
    detail.noteMarkdown = "Refreshed from the canonical database";
    detail.revision += 1;
    detail.updatedAt += 1;
    await window.__CLIPPING_INSTANCE_ACTIVATION__.emit();
  });
  await page.getByLabel("Clipping note editor body").getByText("Refreshed from the canonical database", { exact: true }).waitFor();
  assert.equal(
    await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length),
    callsBeforeActivation + 1,
    "second-launch activation did not reload exactly one selected clipping detail"
  );
  assert.equal(await page.evaluate(() => window.__CLIPPING_INSTANCE_ACTIVATION__.listenerCount()), 2, "activation refresh leaked a Strict Mode listener");
  const callsBeforeSourceRoundTrip = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length);
  await page.locator(".clipping-source-card__media").hover();
  await page.getByRole("button", { name: "Open source newspaper page" }).click();
  await page.locator(".newspaper-reader").waitFor();
  await page.getByRole("button", { name: "Back to clipping" }).waitFor();
  await page.waitForFunction(() => document.querySelector("[aria-label='Select newspaper page']")?.value === "2");
  assert.equal(await page.getByLabel("Select newspaper page").inputValue(), "2", "source navigation used a page index instead of the exact page ID");
  const sourceHighlight = page.locator("[data-testid='newspaper-source-highlight']");
  await sourceHighlight.waitFor();
  const highlightStyle = await sourceHighlight.evaluate((element) => ({
    left: element.style.left,
    top: element.style.top,
    width: element.style.width,
    height: element.style.height,
    pointerEvents: getComputedStyle(element).pointerEvents
  }));
  assert.deepEqual(highlightStyle, { left: "20%", top: "62%", width: "35%", height: "18%", pointerEvents: "none" });
  await page.waitForTimeout(3_150);
  assert.equal(await sourceHighlight.count(), 0, "source highlight did not expire after its three-second display window");
  await page.getByRole("button", { name: "Back to clipping" }).click();
  await page.getByLabel("Clipping note editor body").waitFor();
  assert.equal(await page.getByRole("button", { name: "Open source newspaper page" }).evaluate((element) => element === document.activeElement), true, "source return did not restore focus to Open source");
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.detailCalls.length), callsBeforeSourceRoundTrip + 1, "Back to clipping did not reload the exact clipping ID once");
  await topBack.click();
  await page.locator(".clipping-gallery").waitFor();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByLabel("Clipping note editor body").waitFor();

  const title = page.locator(".clipping-detail__title input");
  await page.evaluate(() => {
    window.__CLIPPING_LIBRARY_TEST__.updateCalls.length = 0;
    window.__CLIPPING_LIBRARY_TEST__.checkpointCalls.length = 0;
  });
  await title.fill("Transit evidence note");
  await title.press("Enter");
  assert.equal(await page.getByLabel("Clipping note editor body").evaluate((element) => element === document.activeElement), true, "Enter in the top-bar title did not focus the note body");
  await page.keyboard.insertText(" with searchable keyword");
  await page.waitForTimeout(950);
  const initialSaveCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.updateCalls);
  assert.ok(initialSaveCalls.length >= 2 && initialSaveCalls.length <= 3, `title/body editing issued ${initialSaveCalls.length} canonical saves: ${JSON.stringify(initialSaveCalls)}`);
  const initialCheckpointCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.checkpointCalls.length);
  assert.ok(initialCheckpointCalls > 0 && initialCheckpointCalls <= 5, `title/body editing issued ${initialCheckpointCalls} recovery checkpoints`);
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
  await page.locator(".clipping-search-row").first().click();
  await page.locator(".clipping-detail").waitFor();
  await page.locator(".clipping-source-card__media").hover();
  await page.getByRole("button", { name: "Open source newspaper page" }).click();
  await page.getByRole("button", { name: "Back to clipping" }).waitFor();
  await page.getByRole("button", { name: "Back to clipping" }).click();
  await page.locator(".clipping-detail").waitFor();
  await topBack.click();
  await page.locator(".clipping-search-results").waitFor();
  assert.equal(await globalSearch.inputValue(), "transit", "source round trip discarded the clipping search query");
  await page.getByLabel("Clear clipping search").click();
  await page.locator(".clipping-gallery").waitFor();
  assert.equal(await page.locator(".clipping-detail").count(), 0, "clearing search bypassed the clipping gallery");
  await page.locator(".clipping-gallery__card").first().click();
  await page.locator(".clipping-detail").waitFor();
  await page.getByText("Saved", { exact: true }).waitFor();
  await page.waitForFunction(() => window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.size === 0);

  const firstConflictCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.updateCalls.length);
  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.conflictNext = true; });
  await title.fill("Local conflict draft");
  await page.waitForFunction(() => document.querySelector(".clipping-detail__title input")?.value === "Local conflict draft");
  await page.waitForFunction(
    (previous) => !window.__CLIPPING_LIBRARY_TEST__.conflictNext && window.__CLIPPING_LIBRARY_TEST__.updateCalls.length > previous,
    firstConflictCalls
  );
  await page.getByText("This note changed in another window.").waitFor();
  assert.equal(await title.inputValue(), "Local conflict draft");
  await page.getByRole("button", { name: "Keep my changes" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();
  await page.waitForFunction(() => window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.size === 0);

  const secondConflictCalls = await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.updateCalls.length);
  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.conflictNext = true; });
  await title.fill("Second local conflict draft");
  await page.waitForFunction(() => document.querySelector(".clipping-detail__title input")?.value === "Second local conflict draft");
  await page.waitForFunction(
    (previous) => !window.__CLIPPING_LIBRARY_TEST__.conflictNext && window.__CLIPPING_LIBRARY_TEST__.updateCalls.length > previous,
    secondConflictCalls
  );
  await page.getByText("This note changed in another window.").waitFor();
  await page.getByRole("button", { name: "Keep my changes" }).click();
  await page.getByText("Saved", { exact: true }).waitFor();
  await page.waitForFunction(() => window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.size === 0);

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
  await page.getByText("Recovered draft saved locally.").waitFor();
  await page.waitForFunction(
    (expected) => [...window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.values()].at(0)?.title === expected,
    "Draft that initially fails",
    { timeout: 3_000 }
  );
  assert.equal(await page.evaluate(() => [...window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.values()].at(0).title), "Draft that initially fails");
  await page.getByRole("button", { name: "Download editions" }).click();
  await page.locator(".newspaper-download").waitFor();
  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  const recoverySearch = page.getByLabel("Search saved newspaper clippings");
  await recoverySearch.fill("Draft that initially fails");
  await page.locator(".clipping-search-results").waitFor();
  assert.equal(await page.getByText("Draft that initially fails", { exact: true }).count(), 0, "recovery-only title entered clipping search");
  await page.getByLabel("Clear clipping search").click();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByText("Recovered unsaved changes").waitFor();
  assert.equal(await title.inputValue(), "Draft that initially fails");
  await page.getByText("Saved", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.size), 0, "canonical recovery did not clear its checkpoint");

  const canonicalBeforeConflict = await title.inputValue();
  await page.evaluate(() => {
    const test = window.__CLIPPING_LIBRARY_TEST__;
    const detail = test.details[0];
    test.recoveryDrafts.set(detail.id, {
      clippingId: detail.id,
      baseRevision: detail.revision - 1,
      writerSessionId: "11111111-1111-4111-8111-111111111111",
      writerSequence: 4,
      title: "Recovered revision conflict",
      markdown: "Newest recovery body",
      updatedAt: Date.now()
    });
  });
  await page.getByRole("button", { name: "Download editions" }).click();
  await page.locator(".newspaper-download").waitFor();
  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByText("This note changed in another window.").waitFor();
  assert.equal(await title.inputValue(), "Recovered revision conflict");
  await page.getByRole("button", { name: "Use saved version" }).click();
  await page.waitForFunction((expected) => document.querySelector(".clipping-detail__title input")?.value === expected, canonicalBeforeConflict);
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.recoveryDrafts.size), 0, "Use saved version did not explicitly discard recovery");

  await page.evaluate(() => {
    const test = window.__CLIPPING_LIBRARY_TEST__;
    const detail = test.details[0];
    test.recoveryDrafts.set(detail.id, {
      clippingId: detail.id,
      baseRevision: detail.revision,
      writerSessionId: "22222222-2222-4222-8222-222222222222",
      writerSequence: 1,
      title: "invalid",
      markdown: "invalid",
      updatedAt: Date.now()
    });
    test.invalidRecoveryIds.add(detail.id);
  });
  await page.getByRole("button", { name: "Download editions" }).click();
  await page.locator(".newspaper-download").waitFor();
  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByText("Recovered changes are invalid. The saved note is untouched.").waitFor();
  assert.equal(await page.getByLabel("Clipping note editor body").count(), 0, "invalid recovery enabled editing before explicit cleanup");
  await page.getByRole("button", { name: "Discard recovered changes" }).click();
  await page.getByLabel("Clipping note editor body").waitFor();
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.invalidRecoveryIds.size), 0);

  const taskProbe = "Task alignment probe";
  const editorBody = page.getByLabel("Clipping note editor body");
  await editorBody.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
  await page.keyboard.type("/todo");
  const todoOption = page.getByRole("option", { name: /^To-do list/ });
  await todoOption.waitFor();
  await todoOption.click();
  await todoOption.waitFor({ state: "detached" });
  await page.keyboard.type(taskProbe);
  const taskItem = editorBody.locator('ul[data-type="taskList"] > li[data-checked]', { hasText: taskProbe });
  await taskItem.waitFor({ timeout: 8_000 }).catch(async (error) => {
    const editorHtml = await editorBody.innerHTML();
    throw new Error(`slash to-do command did not create a task item; editor HTML: ${editorHtml}`, { cause: error });
  });
  const taskGeometry = await taskItem.evaluate((item, expectedText) => {
    const label = item.querySelector(":scope > label");
    const paragraph = item.querySelector(":scope > div > p");
    if (!(label instanceof HTMLElement) || !(paragraph instanceof HTMLParagraphElement)) return null;
    const walker = document.createTreeWalker(paragraph, NodeFilter.SHOW_TEXT);
    let textNode = walker.nextNode();
    while (textNode && !textNode.textContent?.includes(expectedText)) textNode = walker.nextNode();
    if (!textNode) return null;
    const start = Math.max(0, textNode.textContent.indexOf(expectedText));
    const range = document.createRange();
    range.setStart(textNode, start);
    range.setEnd(textNode, Math.min(textNode.textContent.length, start + 1));
    const labelRect = label.getBoundingClientRect();
    const textRect = range.getBoundingClientRect();
    const paragraphStyle = getComputedStyle(paragraph);
    return {
      display: getComputedStyle(item).display,
      paragraphMarginTop: Number.parseFloat(paragraphStyle.marginTop),
      labelTop: labelRect.top,
      labelBottom: labelRect.bottom,
      textTop: textRect.top,
      textBottom: textRect.bottom
    };
  }, taskProbe);
  assert.ok(taskGeometry, "task item did not expose its checkbox and first text line");
  assert.equal(taskGeometry.display, "flex", "task item lost its single-row layout owner");
  assert.equal(taskGeometry.paragraphMarginTop, 0, "task text inherited the generic paragraph top margin");
  assert.ok(
    taskGeometry.labelTop < taskGeometry.textBottom && taskGeometry.labelBottom > taskGeometry.textTop,
    `task checkbox and first text line do not share a row: ${JSON.stringify(taskGeometry)}`
  );
  await page.getByText("Saved", { exact: true }).waitFor();

  const durabilityPerformance = await measureClippingNoteDurabilityBrowser(page, title);
  console.table(durabilityPerformance.exitLatency);
  console.table(durabilityPerformance.editorHeap);

  await page.getByRole("button", { name: "Download editions" }).click();
  await page.locator(".newspaper-download").waitFor();
  assert.equal(await page.locator(".lv-global-search").count(), 0, "clipping search row remained on Download editions");

  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.locator(".clipping-gallery__card").first().click();
  await page.getByLabel("Clipping note editor body").waitFor();
  await page.getByRole("button", { name: "Delete clipping" }).click();
  await page.getByRole("dialog", { name: "Delete this clipping?" }).waitFor();
  await page.getByText("The original newspaper page is not deleted.", { exact: false }).waitFor();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(await page.locator(".clipping-detail").count(), 1, "cancelling deletion removed the clipping detail");
  await page.getByRole("button", { name: "Delete clipping" }).click();
  await page.getByRole("dialog", { name: "Delete this clipping?" }).getByRole("button", { name: "Delete clipping" }).click();
  await page.locator(".clipping-detail").waitFor();
  await page.waitForFunction(() => document.querySelector(".clipping-detail__title input")?.value === "Transit archive clipping 2");
  assert.equal(await title.inputValue(), "Transit archive clipping 2", "confirmed deletion did not select the next cached clipping");
  assert.equal(await page.evaluate(() => window.__CLIPPING_LIBRARY_TEST__.deleteCalls.length), 1, "confirmed deletion did not issue one revision-guarded request");

  await page.evaluate(() => { window.__CLIPPING_LIBRARY_TEST__.details = []; });
  await page.getByRole("button", { name: "Clippings", exact: true }).click();
  await page.getByRole("heading", { name: "No clippings yet" }).waitFor();
  assert.equal(await page.locator(".clipping-gallery__empty-card").count(), 1, "empty gallery does not show the refined empty card");
  assert.equal(await page.locator(".clipping-gallery__skeletons:not(.is-loading)").count(), 0, "empty gallery still shows decorative skeletons");
  await page.getByText("Save clips from Newspaper library and they will show up here with their notes.", { exact: true }).waitFor();
  await page.getByRole("button", { name: "List view" }).waitFor();
  await page.getByRole("button", { name: "Gallery view" }).waitFor();
  await page.getByText("0 clippings", { exact: true }).waitFor();
  if (process.env.LINKVAULT_CLIPPING_EMPTY_SCREENSHOT) {
    await page.screenshot({ path: process.env.LINKVAULT_CLIPPING_EMPTY_SCREENSHOT });
  }
  await page.getByRole("button", { name: "Open Newspaper library" }).click();
  await page.locator(".newspaper-library").waitFor();

  assert.deepEqual(consoleErrors, [], `browser console/page errors: ${consoleErrors.join("\n")}`);
  console.log("Clipping library browser matrix passed: compact search-row summary, responsive full-snapshot gallery, metadata below pixels, first-use skeletons, lazy thumbnails/detail/editor, autosave, recovery, native preparation, search, conflict, roots, and guarded navigation.");
} finally {
  await browser.close();
}
