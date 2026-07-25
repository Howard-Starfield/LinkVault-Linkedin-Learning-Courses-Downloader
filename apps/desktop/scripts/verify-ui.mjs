import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const app = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
const view = await readFile(new URL("../src/components/newspaper/NewspaperView.tsx", import.meta.url), "utf8");
const css = await readFile(new URL("../src/index.css", import.meta.url), "utf8");

for (const required of [
  "Download editions",
  "Newspaper library",
  'activeView === "newspaper-download"',
  'activeView === "newspaper-library"',
  "Switch to ${theme === \"dark\" ? \"day\" : \"night\"} mode"
]) {
  assert.ok(app.includes(required), `App shell is missing: ${required}`);
}

for (const required of [
  "Delay between editions",
  "High clarity · WebP 92",
  "Balanced · WebP 86",
  "Keep original JPG files",
  'schedule ? "Schedule downloads" : "Download now"',
  "Register archive",
  "All statuses",
  "Select newspaper page",
  "Fit page width",
  "Cancel batch",
  "get_newspaper_preview",
  "get_newspaper_page_image"
]) {
  assert.ok(view.includes(required), `Newspaper view is missing: ${required}`);
}

assert.ok(css.includes("grid-template-columns: minmax(0, 1.7fr) minmax(310px, 1fr)"), "Desktop newspaper setup must be two columns.");
assert.ok(css.includes(".newspaper-editions,\n.newspaper-options"), "Both setup columns must share the same height rule.");
assert.ok(css.includes("@media (max-width: 980px)"), "Newspaper view needs the approved responsive breakpoint.");
assert.ok(!view.includes("<h1") && !view.includes("<h2"), "Downloader must not render a redundant page or section heading.");

console.log("UI contract verification passed.");
