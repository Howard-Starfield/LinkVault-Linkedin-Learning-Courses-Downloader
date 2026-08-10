import assert from "node:assert/strict";
import { ClippingNoteExitPreparation } from "../src/components/newspaper/clipping-note-exit-preparation.ts";

{
  const preparation = new ClippingNoteExitPreparation();
  let releaseFlush;
  let flushes = 0;
  const flush = () => {
    flushes += 1;
    return new Promise((resolve) => { releaseFlush = resolve; });
  };
  const firstClose = preparation.prepare(flush);
  const duplicateClose = preparation.prepare(flush);
  const overlappingQuit = preparation.prepare(flush);
  await Promise.resolve();
  assert.equal(flushes, 1, "overlapping lifecycle requests did not share one exact flush");
  releaseFlush(true);
  assert.deepEqual(await Promise.all([firstClose, duplicateClose, overlappingQuit]), [true, true, true]);
}

{
  const preparation = new ClippingNoteExitPreparation();
  assert.equal(await preparation.prepare(async () => false), false);
  assert.equal(await preparation.prepare(async () => { throw new Error("offline"); }), false);
  assert.equal(await preparation.prepare(null), true);
}

console.log("Clipping note renderer exit preparation coalescing and failure contracts passed.");
