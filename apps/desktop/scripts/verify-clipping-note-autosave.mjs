import assert from "node:assert/strict";
import { ClippingNoteCheckpointController } from "../src/components/newspaper/clipping-note-checkpoint-controller.ts";
import { ClippingNoteSaveController } from "../src/components/newspaper/clipping-note-save-controller.ts";
import { preserveStableClippingThumbnail } from "../src/components/newspaper/clipping-thumbnail-state.ts";

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const initial = { documentId: "clip-1", title: "Initial", markdown: "", revision: 5 };
const code = (error) => String(error);

function createFakeTimers() {
  let now = 0;
  let nextId = 1;
  const tasks = new Map();
  return {
    scheduler: {
      schedule(callback, delayMs) {
        const id = nextId;
        nextId += 1;
        tasks.set(id, { at: now + delayMs, callback });
        return id;
      },
      cancel(handle) {
        tasks.delete(handle);
      }
    },
    advance(milliseconds) {
      const target = now + milliseconds;
      while (true) {
        const due = [...tasks.entries()]
          .filter(([, task]) => task.at <= target)
          .sort((left, right) => left[1].at - right[1].at || left[0] - right[0])[0];
        if (!due) break;
        const [id, task] = due;
        tasks.delete(id);
        now = task.at;
        task.callback();
      }
      now = target;
    }
  };
}

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
  const fakeTimers = createFakeTimers();
  const calls = [];
  const controller = new ClippingNoteSaveController(initial, async (request) => {
    calls.push(request);
    return { ...request, revision: request.expectedRevision + 1 };
  }, code, 10, () => null, 25, fakeTimers.scheduler);
  controller.setTitle("First continuous draft");
  fakeTimers.advance(9);
  controller.setTitle("Second continuous draft");
  fakeTimers.advance(9);
  controller.setTitle("Latest continuous draft");
  fakeTimers.advance(6);
  assert.equal(calls.length, 0, "continuous typing saved before the maximum wait");
  fakeTimers.advance(1);
  await Promise.resolve();
  assert.equal(calls.length, 1, "maximum wait did not bound continuous typing");
  assert.equal(calls[0].title, "Latest continuous draft");
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
  }, code, 1_000);
  controller.setMarkdown("flush first");
  const flushing = controller.flush();
  await Promise.resolve();
  controller.setMarkdown("flush newest visible draft");
  releases.shift()();
  await wait(0);
  assert.equal(calls.length, 2, "flush returned before submitting queued-latest work");
  assert.equal(calls[1].markdown, "flush newest visible draft");
  releases.shift()();
  assert.equal(await flushing, true);
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
  second.setMarkdown("newest visible conflict draft");
  assert.equal(second.getSnapshot().status, "conflict");
  assert.equal(second.getSnapshot().draftMarkdown, "newest visible conflict draft");
  assert.equal(await second.keepMyChanges({ ...shared }), true);
  assert.equal(shared.revision, 7);
  assert.equal(shared.markdown, "newest visible conflict draft");
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

{
  const fakeTimers = createFakeTimers();
  const calls = [];
  const controller = new ClippingNoteCheckpointController(
    "clip-1",
    "session-a",
    5,
    "Initial",
    "",
    async (request) => {
      calls.push(request);
      return {
        documentId: request.documentId,
        writerSessionId: request.writerSessionId,
        writerSequence: request.writerSequence
      };
    },
    code,
    10,
    25,
    () => null,
    fakeTimers.scheduler
  );
  controller.setDraft(5, "First checkpoint", "one");
  fakeTimers.advance(9);
  controller.setDraft(5, "Second checkpoint", "two");
  fakeTimers.advance(9);
  controller.setDraft(5, "Latest checkpoint", "three");
  fakeTimers.advance(7);
  await Promise.resolve();
  assert.equal(calls.length, 1, "checkpoint maximum wait did not coalesce continuous typing");
  assert.equal(calls[0].title, "Latest checkpoint");
  assert.equal(controller.getSnapshot().status, "durable");
  assert.equal(controller.getSnapshot().durableSequence, 3);
  controller.dispose();
}

{
  const calls = [];
  const releases = [];
  const controller = new ClippingNoteCheckpointController(
    "clip-1",
    "session-b",
    5,
    "Initial",
    "",
    (request) => {
      calls.push(request);
      return new Promise((resolve) => releases.push(() => resolve({
        documentId: request.documentId,
        writerSessionId: request.writerSessionId,
        writerSequence: request.writerSequence
      })));
    },
    code,
    1_000,
    2_000
  );
  controller.setDraft(5, "Initial", "checkpoint first");
  const ensuring = controller.ensureDurable();
  await Promise.resolve();
  controller.setDraft(5, "Initial", "checkpoint newest");
  releases.shift()();
  await wait(0);
  assert.equal(calls.length, 2, "durability acknowledgement skipped queued-latest checkpoint work");
  assert.equal(calls[1].markdown, "checkpoint newest");
  releases.shift()();
  assert.equal(await ensuring, true);
  assert.equal(controller.getSnapshot().durableSequence, 2);
  controller.dispose();
}

{
  const controller = new ClippingNoteCheckpointController(
    "clip-1",
    "session-c",
    5,
    "Initial",
    "",
    async () => { throw new Error("CLIPPING_RECOVERY_WRITER_CONFLICT"); },
    (error) => error.message,
    1_000,
    2_000
  );
  controller.setDraft(5, "Initial", "conflicted checkpoint");
  assert.equal(await controller.ensureDurable(), false);
  assert.equal(controller.getSnapshot().status, "conflict");
  controller.setDraft(5, "Still local", "newest visible conflict checkpoint");
  assert.equal(controller.getSnapshot().status, "conflict");
  assert.equal(controller.getSnapshot().draftMarkdown, "newest visible conflict checkpoint");
  assert.equal(controller.getSnapshot().writerSequence, 2);
  controller.dispose();
}

{
  const controller = new ClippingNoteCheckpointController(
    "clip-1",
    "session-d",
    5,
    "Initial",
    "",
    async (request) => ({
      documentId: request.documentId,
      writerSessionId: request.writerSessionId,
      writerSequence: request.writerSequence + 1
    }),
    code,
    1_000,
    2_000
  );
  controller.setDraft(5, "Canonical next", "canonical next");
  assert.equal(await controller.ensureDurable(), false, "a future-sequence acknowledgement was accepted");
  assert.equal(controller.getSnapshot().errorCode, "CLIPPING_RECOVERY_STALE_ACK");
  assert.equal(controller.acknowledgeCanonicalSave("wrong-session", 1, 6), false);
  assert.equal(controller.acknowledgeCanonicalSave("session-d", 1, 6), true);
  assert.equal(controller.getSnapshot().status, "idle");
  controller.dispose();
}

console.log("Clipping note canonical autosave, checkpoint coalescing, stable thumbnails, flush, retry, queued-latest, validation, and conflict contracts passed.");
