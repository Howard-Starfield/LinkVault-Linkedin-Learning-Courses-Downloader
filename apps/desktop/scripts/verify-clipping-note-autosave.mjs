import assert from "node:assert/strict";
import { ClippingNoteSaveController } from "../src/components/newspaper/clipping-note-save-controller.ts";
import { preserveStableClippingThumbnail } from "../src/components/newspaper/clipping-thumbnail-state.ts";

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const initial = { documentId: "clip-1", title: "Initial", markdown: "", revision: 5 };
const code = (error) => String(error);

{
  const cached = {
    id: "clip-1",
    assetState: "ready",
    assetVersion: 3,
    thumbnailReady: true,
    thumbnailUrl: "http://newspaper-media.localhost/clipping-thumbnail/clip-1?v=3-1",
    thumbnailVersion: "3-1",
    revision: 4
  };
  const refreshed = preserveStableClippingThumbnail({
    ...cached,
    thumbnailReady: false,
    thumbnailUrl: null,
    thumbnailVersion: null,
    revision: 5
  }, cached);
  assert.equal(refreshed.thumbnailReady, true, "note invalidation regressed a proven thumbnail to its placeholder");
  assert.equal(refreshed.revision, 5, "thumbnail preservation discarded fresh canonical list data");
  assert.equal(preserveStableClippingThumbnail({ ...refreshed, assetVersion: 4, thumbnailReady: false }, cached).thumbnailReady, false);
  assert.equal(preserveStableClippingThumbnail({ ...refreshed, assetState: "missing", thumbnailReady: false }, cached).thumbnailReady, false);
}

{
  const calls = [];
  const controller = new ClippingNoteSaveController(initial, async (request) => {
    calls.push(request);
    return { documentId: request.documentId, title: request.title.trim(), markdown: request.markdown, revision: 6 };
  }, code, 20);
  controller.setTitle("First");
  await wait(8);
  controller.setTitle("Latest");
  await wait(12);
  assert.equal(calls.length, 0, "autosave fired before 800ms-equivalent stability window");
  await wait(20);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].title, "Latest");
  assert.equal(controller.getSnapshot().status, "clean");
  controller.dispose();
}

{
  const calls = [];
  const releases = [];
  const controller = new ClippingNoteSaveController(initial, (request) => {
    calls.push(request);
    return new Promise((resolve) => releases.push(() => resolve({
      documentId: request.documentId,
      title: request.title,
      markdown: request.markdown,
      revision: request.expectedRevision + 1
    })));
  }, code, 1);
  controller.setMarkdown("first draft");
  await wait(5);
  assert.equal(calls.length, 1);
  controller.setMarkdown("queued latest");
  releases.shift()();
  await wait(8);
  assert.equal(calls.length, 2, "queued-latest work must begin only after acknowledgement");
  assert.equal(calls[1].markdown, "queued latest");
  assert.equal(calls[1].expectedRevision, 6);
  releases.shift()();
  await wait(2);
  assert.equal(controller.getSnapshot().status, "clean");
  controller.dispose();
}

{
  let release;
  const calls = [];
  const controller = new ClippingNoteSaveController(initial, (request) => {
    calls.push(request);
    return new Promise((resolve) => {
      release = () => resolve({ ...initial, revision: 6 });
    });
  }, code, 1);
  controller.setMarkdown("temporary");
  await wait(5);
  controller.setMarkdown("");
  release();
  await wait(5);
  assert.equal(calls.length, 1, "returning to persisted bytes must not issue a redundant second save");
  assert.equal(controller.getSnapshot().status, "clean");
  controller.dispose();
}

{
  let fail = true;
  const drafts = [];
  const controller = new ClippingNoteSaveController(initial, async (request) => {
    drafts.push(request.markdown);
    if (fail) throw new Error("CLIPPING_DATABASE_WRITE_FAILED");
    return { ...initial, markdown: request.markdown, revision: 6 };
  }, (error) => error.message, 1);
  controller.setMarkdown("preserved after failure");
  await wait(5);
  assert.equal(controller.getSnapshot().status, "failed");
  assert.equal(controller.getSnapshot().draftMarkdown, "preserved after failure");
  fail = false;
  assert.equal(await controller.retry(), true);
  assert.equal(controller.getSnapshot().status, "clean");
  assert.deepEqual(drafts, ["preserved after failure", "preserved after failure"]);
  controller.dispose();
}

{
  const shared = { ...initial };
  const save = async (request) => {
    if (request.expectedRevision !== shared.revision) throw new Error("CLIPPING_REVISION_CONFLICT");
    shared.title = request.title;
    shared.markdown = request.markdown;
    shared.revision += 1;
    return { ...shared };
  };
  const first = new ClippingNoteSaveController(initial, save, (error) => error.message, 1000);
  const second = new ClippingNoteSaveController(initial, save, (error) => error.message, 1000);
  first.setMarkdown("winner");
  assert.equal(await first.flush(), true);
  second.setMarkdown("local draft");
  assert.equal(await second.flush(), false);
  assert.equal(second.getSnapshot().status, "conflict");
  assert.equal(second.getSnapshot().draftMarkdown, "local draft");
  assert.equal(await second.keepMyChanges({ ...shared }), true);
  assert.equal(shared.revision, 7);
  assert.equal(shared.markdown, "local draft");
  first.dispose();
  second.dispose();
}

{
  let calls = 0;
  const controller = new ClippingNoteSaveController(initial, async () => {
    calls += 1;
    return initial;
  }, code, 1, ({ title }) => title.trim() ? null : "CLIPPING_INVALID_TITLE");
  controller.setTitle("   ");
  await wait(5);
  assert.equal(calls, 0);
  assert.equal(await controller.flush(), false);
  assert.equal(controller.getSnapshot().draftTitle, "   ");
  controller.dispose();
}

console.log("Clipping note autosave, stable thumbnails, flush, retry, queued-latest, validation, and conflict contracts passed.");
