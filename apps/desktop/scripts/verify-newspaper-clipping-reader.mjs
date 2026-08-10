import assert from "node:assert/strict";
import {
  clampClientPoint,
  clientRectSizesMateriallyDiffer,
  estimateSourceCropSize,
  isEstimatedCropLargeEnough,
  normalizedCropRectFromClientPoints
} from "../src/components/newspaper/newspaper-clipping-geometry.ts";
import {
  initialNewspaperClippingInteraction,
  newspaperClippingReducer
} from "../src/components/newspaper/newspaper-clipping-state.ts";

const image = { left: 100.25, top: 40.5, width: 800.5, height: 1200.75 };
assert.deepEqual(clampClientPoint({ x: -20, y: 2000 }, image), { x: 100.25, y: 1241.25 });
assert.deepEqual(clampClientPoint({ x: -20, y: -20 }, image), { x: 100.25, y: 40.5 });
assert.deepEqual(clampClientPoint({ x: 2000, y: -20 }, image), { x: 900.75, y: 40.5 });
assert.deepEqual(clampClientPoint({ x: 2000, y: 2000 }, image), { x: 900.75, y: 1241.25 });

const expected = { x: .25, y: .25, width: .5, height: .5 };
for (const [start, end] of [
  [{ x: 300.375, y: 340.6875 }, { x: 700.625, y: 941.0625 }],
  [{ x: 700.625, y: 340.6875 }, { x: 300.375, y: 941.0625 }],
  [{ x: 300.375, y: 941.0625 }, { x: 700.625, y: 340.6875 }],
  [{ x: 700.625, y: 941.0625 }, { x: 300.375, y: 340.6875 }]
]) {
  assert.deepEqual(normalizedCropRectFromClientPoints(start, end, image), expected);
}
assert.deepEqual(
  normalizedCropRectFromClientPoints({ x: -100, y: -100 }, { x: 2000, y: 3000 }, image),
  { x: 0, y: 0, width: 1, height: 1 }
);
for (const scale of [.5, 1, 1.25, 1.5, 2, 3]) {
  const scaled = {
    left: 37.125,
    top: -22.75,
    width: 640 * scale,
    height: 960 * scale
  };
  assert.deepEqual(
    normalizedCropRectFromClientPoints(
      { x: scaled.left + scaled.width * .125, y: scaled.top + scaled.height * .2 },
      { x: scaled.left + scaled.width * .875, y: scaled.top + scaled.height * .8 },
      scaled
    ),
    { x: .125, y: .2, width: .75, height: .6 }
  );
}
assert.equal(normalizedCropRectFromClientPoints({ x: 200, y: 200 }, { x: 200, y: 300 }, image), null);
assert.equal(normalizedCropRectFromClientPoints({ x: Number.NaN, y: 0 }, { x: 1, y: 1 }, image), null);
assert.equal(normalizedCropRectFromClientPoints({ x: 0, y: 0 }, { x: 1, y: 1 }, { left: 0, top: 0, width: 0, height: 1 }), null);
assert.deepEqual(estimateSourceCropSize({ x: .25, y: .25, width: .5, height: .5 }, 2501, 4001), {
  width: 1251,
  height: 2001
});
assert.equal(isEstimatedCropLargeEnough({ width: 31, height: 32 }), false);
assert.equal(isEstimatedCropLargeEnough(null), true);
assert.equal(clientRectSizesMateriallyDiffer(image, { ...image, left: image.left + 100 }), false);
assert.equal(clientRectSizesMateriallyDiffer(image, { ...image, width: image.width + .1 }), false);
assert.equal(clientRectSizesMateriallyDiffer(image, { ...image, width: image.width + 1 }), true);
assert.equal(clientRectSizesMateriallyDiffer(image, { ...image, height: image.height - 1 }), true);

let state = newspaperClippingReducer(initialNewspaperClippingInteraction, { type: "ENTER" });
assert.equal(state.type, "clip-selecting");
assert.equal(newspaperClippingReducer(initialNewspaperClippingInteraction, { type: "REJECT_SMALL" }), initialNewspaperClippingInteraction);
assert.equal(newspaperClippingReducer(state, { type: "REDRAW" }), state);
state = newspaperClippingReducer(state, {
  type: "START",
  pointerId: 7,
  pageId: "page-1",
  pageIndex: 0,
  expectedMediaVersion: 2,
  rect: expected,
  estimatedSize: { width: 100, height: 100 }
});
assert.equal(state.type, "clip-drawing");
state = newspaperClippingReducer(state, { type: "CONFIRM", rect: expected, estimatedSize: { width: 100, height: 100 } });
assert.equal(state.type, "clip-confirming");
state = newspaperClippingReducer(state, { type: "SAVE", operationId: "operation-1" });
assert.equal(state.type, "clip-saving");
state = newspaperClippingReducer(state, { type: "SAVE_FAILED", error: "Retry", retainOperationId: true });
assert.equal(state.type, "clip-confirming");
assert.equal(state.operationId, "operation-1");
state = newspaperClippingReducer(state, { type: "SAVE", operationId: "operation-1" });
assert.equal(state.type, "clip-saving");
state = newspaperClippingReducer(state, {
  type: "REFRESHED",
  pageId: "page-1",
  pageIndex: 0,
  expectedMediaVersion: 3,
  rect: expected,
  estimatedSize: { width: 100, height: 100 },
  announcement: "Redraw required"
});
assert.equal(state.type, "clip-confirming");
assert.equal(state.requiresRedraw, true);
assert.equal(newspaperClippingReducer(state, { type: "SAVE", operationId: "operation-2" }), state);
assert.equal(newspaperClippingReducer(state, { type: "CANCEL" }).type, "browse");

console.log("newspaper clipping reader geometry/state verification passed");
