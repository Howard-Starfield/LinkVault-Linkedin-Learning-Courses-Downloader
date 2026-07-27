import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  boundedLibraryRange,
  pageOffsetsForRange,
  threePageRange,
  visibleVirtualIndexes
} from "../src/components/newspaper/newspaper-virtualization.ts";
import {
  DEFAULT_NEWSPAPER_READER_ZOOM,
  clampNewspaperReaderZoom,
  readNewspaperReaderPreferences
} from "../src/components/newspaper/newspaper-reader-preferences.ts";

assert.equal(DEFAULT_NEWSPAPER_READER_ZOOM, 1, "Fresh readers must open at 100 percent");
assert.equal(clampNewspaperReaderZoom(Number.NaN), 1, "Invalid zoom preferences must use the safe default");
assert.equal(clampNewspaperReaderZoom(.1), .5, "Reader zoom must retain its lower bound");
assert.equal(clampNewspaperReaderZoom(4), 3, "Reader zoom must retain its upper bound");
assert.deepEqual(
  readNewspaperReaderPreferences(),
  { defaultZoom: 1, clickZoom: 1.2, pageTone: "soft" },
  "Non-browser verification must receive safe reader defaults"
);

for (const pageCount of [8, 50, 500]) {
  for (let activeIndex = 0; activeIndex < pageCount; activeIndex += 1) {
    const mounted = threePageRange(activeIndex, pageCount);
    assert.ok(mounted.length <= 3, `${pageCount}-page reader mounted more than three pages`);
    assert.ok(mounted.includes(activeIndex), "Reader range omitted the active page");
    assert.deepEqual([...mounted].sort((a, b) => a - b), mounted, "Reader range must stay ordered");
  }
  assert.equal(threePageRange(0, pageCount).length, 2, "First page should mount only current and next");
  assert.equal(threePageRange(pageCount - 1, pageCount).length, 2, "Last page should mount only previous and current");
}

for (const editionCount of [8, 50, 500]) {
  const visibleRows = 8;
  const overscan = 4;
  const firstVisible = Math.max(0, Math.floor(editionCount / 2) - 4);
  const mountedRows = boundedLibraryRange(firstVisible, visibleRows, editionCount, overscan);
  assert.ok(
    mountedRows.length <= visibleRows + overscan * 2,
    `${editionCount}-edition library mounted an unbounded row count`
  );
  const pageOffsets = pageOffsetsForRange(mountedRows, 50);
  assert.ok(pageOffsets.length <= 2, `${editionCount}-edition viewport requested more than two 50-row pages`);
}

const visibleRowsOnly = visibleVirtualIndexes(
  Array.from({ length: 16 }, (_, index) => ({
    index,
    start: (index - 4) * 112,
    end: (index - 3) * 112
  })),
  0,
  8 * 112
);
assert.deepEqual(
  [...visibleRowsOnly],
  Array.from({ length: 8 }, (_, index) => index + 4),
  "Thumbnail eligibility must exclude overscanned rows outside the viewport"
);

const readerSource = await readFile(
  new URL("../src/components/newspaper/NewspaperReader.tsx", import.meta.url),
  "utf8"
);
const librarySource = await readFile(
  new URL("../src/components/newspaper/NewspaperLibrary.tsx", import.meta.url),
  "utf8"
);
const commandsSource = await readFile(
  new URL("../src-tauri/src/providers/newspaper/commands.rs", import.meta.url),
  "utf8"
);

assert.ok(readerSource.includes("rangeExtractor"), "Reader must supply its bounded range extractor");
assert.ok(
  readerSource.includes("(range.startIndex + range.endIndex) / 2"),
  "Reader range extraction must follow fast visible-range jumps"
);
assert.ok(readerSource.includes("overscan: 0"), "Reader must not add hidden image overscan");
assert.ok(readerSource.includes('loading="eager"'), "All bounded reader images must preload before they enter view");
assert.ok(readerSource.includes("const PAGE_GAP = 2"), "Reader page seam must remain a hairline");
assert.ok(readerSource.includes("panGestureRef"), "Reader panning must stay on the stable virtual scroll container");
assert.ok(readerSource.includes("data-page-tone"), "Virtual Reader pages must inherit one root-level tone");
assert.ok(librarySource.includes("PAGE_SIZE = 50"), "Library queries must remain paged");
assert.ok(librarySource.includes("overscan: 4"), "Library row overscan contract changed unexpectedly");
assert.ok(
  librarySource.includes("prefetchOffset < total && !items[prefetchOffset]"),
  "Sparse deep Library scrolling must not reload an already populated page"
);
assert.ok(
  librarySource.includes("visibleIndexes.has(virtualItem.index)"),
  "Library thumbnail generation must stay limited to visible rows"
);
assert.ok(!commandsSource.includes("get_newspaper_page_image"), "Legacy base64 reader IPC is still present");
assert.ok(!commandsSource.includes("get_newspaper_preview"), "Legacy base64 thumbnail IPC is still present");

console.log("Newspaper virtualization scale contracts passed for 8, 50, and 500 editions/pages.");
