import assert from "node:assert/strict";
import { normalizeClippingNoteMarkdown } from "../src/components/newspaper/clipping-note-editor-markdown.ts";
import { CLIPPING_NOTE_MDX_EDGE_CASES_FIXTURE } from "../src/editor-evaluation/fixtures.ts";

const normalized = normalizeClippingNoteMarkdown(CLIPPING_NOTE_MDX_EDGE_CASES_FIXTURE);
const expected = `Before (foo: (bar: 1)) after

Before import
After import

Before export
After export

Before table
After table

Before short table
After short table

Before prose
import findings from yesterday
[Preserved link](https://example.com/explicit)
After prose

Before export prose
export const findings from yesterday
After export prose`;

assert.equal(normalized, expected, "the exact multiline MDX and pipe-less table fixture must reduce to inert V1 prose");
for (const fragment of ["{", "}", "import {", "thing", "export const x =", "value:1", "a | b", "--- | ---", "1 | 2", "c | d", "- | -", "3 | 4"]) {
  assert.ok(!normalized.includes(fragment), `unsupported source fragment survived normalization: ${fragment}`);
}
assert.ok(normalized.includes("import findings from yesterday"), "ordinary prose beginning with import was removed");
assert.ok(normalized.includes("export const findings from yesterday"), "ordinary prose beginning with export was removed");
assert.ok(normalized.includes("[Preserved link](https://example.com/explicit)"), "safe explicit link was removed");

console.log(JSON.stringify({
  status: "pass",
  fixture: "nested MDX, multiline import/export, pipe-less GFM table",
  normalizedLength: normalized.length
}, null, 2));
