import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const app = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
const view = await readFile(new URL("../src/components/newspaper/NewspaperView.tsx", import.meta.url), "utf8");
const library = await readFile(new URL("../src/components/newspaper/NewspaperLibrary.tsx", import.meta.url), "utf8");
const reader = await readFile(new URL("../src/components/newspaper/NewspaperReader.tsx", import.meta.url), "utf8");
const readerPreferences = await readFile(new URL("../src/components/newspaper/newspaper-reader-preferences.ts", import.meta.url), "utf8");
const newspaperApi = await readFile(new URL("../src/components/newspaper/newspaper-api.ts", import.meta.url), "utf8");
const css = await readFile(new URL("../src/index.css", import.meta.url), "utf8");

for (const required of [
  "Download editions",
  "Newspaper library",
  'activeView === "newspaper-download"',
  'activeView === "newspaper-library"',
  "processNewspaperSchedules",
  'invoke("process_newspaper_queue")',
  '"process_newspaper_optimization_queue"',
  "Default zoom level",
  "Left-click zoom level",
  "Default newspaper page tone",
  "Register archive",
  "Repair existing",
  "windowResizing",
  'window.addEventListener("resize", handleWindowResize, { passive: true })',
  "Switch to ${theme === \"dark\" ? \"day\" : \"night\"} mode",
  "Data management",
  "Reset LinkedIn database",
  "Reset Coursera database",
  "Reset World Journal database",
  "reset_linkedin_database",
  "reset_coursera_database",
  "reset_newspaper_database",
  "performProviderReset",
  "pausingForReset"
]) {
  assert.ok(app.includes(required), `App shell is missing: ${required}`);
}

for (const required of [
  "Select newspaper editions",
  "Regional",
  "Weekly",
  "Special",
  "Daily schedule",
  "Add another time",
  "command-actions newspaper-schedule-submit",
  "History",
  "System current date",
  "Delay between editions",
  "Save location",
  "Image compression strength",
  "compressionLabel",
  "High clarity · WebP 92",
  "Compact · WebP 45",
  "Very small · WebP 35",
  "Maximum savings · WebP 25",
  "Keep source JPG",
  "JPG remains only if WebP is larger or fails",
  "Download now",
  "command-actions newspaper-download-actions",
  "seconds",
  "Awaiting release",
  "Optimization worker mode",
  "Manual ceiling",
  "newspaper://optimization-progress",
  "Downloaded",
  "Optimized",
  "Newspaper",
  "set_newspaper_job_pause",
  "set_all_newspaper_jobs_paused",
  "toggleAllNewspaperJobsPause",
  "isNewspaperQueueRunning",
  "Pause all",
  "Resume all",
  "All newspaper downloads paused",
  "reorder_newspaper_jobs",
  "remove_newspaper_job",
  "Drag to reorder",
  "Permanently delete this downloaded edition?",
  "local files and progress history were removed",
  "Pause this download before deleting it",
  'disabled={["active", "optimizing"].includes(job.status)}',
  'role="progressbar"',
]) {
  assert.ok(view.includes(required), `Newspaper view is missing: ${required}`);
}
assert.ok(!view.includes("92 - Math.round"), "Quality labels and submitted values must use the same explicit preset.");

for (const required of [
  "All statuses",
  "useVirtualizer",
  "PAGE_SIZE = 50",
  "overscan: 4",
  "ensureThumbnail",
  "writeNewspaperReaderPreferences",
  'className="newspaper-library-open"',
  'className="newspaper-reading-progress"',
  'loading="lazy"',
  'decoding="async"'
]) {
  assert.ok(library.includes(required), `Virtual newspaper library is missing: ${required}`);
}

for (const required of [
  "Select newspaper page",
  "Fit page width",
  "Back to library",
  "createPortal",
  "useVirtualizer",
  "threePageRange",
  "activeIndex - 1",
  "activeIndex + 1",
  "overscan: 0",
  "rangeExtractor",
  "PAN_DRAG_THRESHOLD",
  "clickZoom",
  "setPointerCapture",
  "startScrollLeft - deltaX",
  "startScrollTop - deltaY",
  'data-pan-enabled={panEnabled ? "true" : "false"}',
  'data-panning={isPanning ? "true" : "false"}',
  'data-mounted-page-images',
  'data-testid="newspaper-reader-page-image"',
  'data-click-zoomed={isClickZoomed ? "true" : undefined}',
  "xRatio: (event.clientX - rect.left)",
  "element.style.scrollBehavior = \"auto\"",
  'loading="eager"',
  'decoding="async"',
  "saveReadingProgress"
]) {
  assert.ok(reader.includes(required), `Continuous newspaper reader is missing: ${required}`);
}
assert.ok(readerPreferences.includes("DEFAULT_NEWSPAPER_READER_ZOOM = 1"), "Reader default zoom must remain 100 percent.");
assert.ok(readerPreferences.includes("NEWSPAPER_READER_PREFERENCES_VERSION = 3"), "Reader preferences must include the configurable click zoom.");
assert.ok(readerPreferences.includes("DEFAULT_NEWSPAPER_CLICK_ZOOM = 1.2"), "Left click must default to 120 percent.");
assert.ok(readerPreferences.includes('pageTone: "soft"'), "Reader must default to the low-glare soft paper tone.");
assert.ok(readerPreferences.includes("window.localStorage.setItem"), "Reader preferences must persist across sessions.");

assert.equal((app.match(/className="lv-sidebar-reopen"/g) ?? []).length, 1, "The shared main surface must own one sidebar reopen control.");
assert.ok(app.indexOf('className="lv-sidebar-reopen"') < app.indexOf('className="lv-content"'), "The sidebar reopen control must not be scoped to one provider view.");
assert.ok(!library.includes(">Read</Button>"), "Opening a newspaper must be owned by the whole library row.");
assert.ok(!library.includes("Register archive") && !library.includes("Repair existing"), "Archive maintenance belongs in Settings, not the Library toolbar.");
assert.ok(!library.includes("Default newspaper zoom"), "Reader defaults belong in Settings, not the Library toolbar.");
assert.ok(library.includes("item.readPageCount"), "Library progress must use unique viewed-page coverage.");
assert.ok(!library.includes("item.furthestPageIndex + 1"), "Library progress must not treat the furthest reached page as read coverage.");
assert.ok(!newspaperApi.includes("get_newspaper_preview"), "Thumbnail transport must not use the legacy base64 IPC command.");
assert.ok(!newspaperApi.includes("get_newspaper_page_image"), "Reader transport must not use the legacy base64 IPC command.");
assert.ok(css.includes(".newspaper-dispatch-grid"), "Newspaper view needs the three-panel dispatch grid.");
assert.ok(css.includes("minmax(280px, 0.96fr) minmax(340px, 1.08fr) minmax(300px, 0.98fr)"), "Wide newspaper workspace must render three durable panels.");
assert.ok(css.includes('grid-template-areas: "editions settings schedule"'), "Download settings must sit between editions and schedule.");
assert.ok(css.includes("minmax(380px, 0.64fr) minmax(240px, 0.56fr)"), "Compact dispatch row must reserve useful height for Progress.");
assert.ok(css.includes(".newspaper-progress-actions") && css.includes("opacity: 0"), "Queue actions must reveal on hover or focus.");
assert.ok(css.includes("@media (max-width: 1120px)"), "Newspaper view needs the approved responsive collapse.");
assert.ok(css.includes("width: 100vw") && css.includes("height: 100vh"), "Reader must occupy the full window.");
assert.ok(css.includes("conic-gradient(var(--accent) var(--reading-progress)"), "Reading progress needs the circular library indicator.");
assert.ok(css.includes(".newspaper-library-toolbar .newspaper-search input") && css.includes("height: 2.25rem"), "Library search must match adjacent control height.");
assert.ok(css.includes('data-page-tone="soft"') && css.includes(".newspaper-reader-tone-overlay"), "Soft paper must use one geometry-neutral viewport treatment.");
assert.ok(css.includes("pointer-events: none"), "The page-tone overlay must not intercept zoom, drag, or scroll input.");
assert.ok(css.includes('data-pan-enabled="true"') && css.includes("cursor: grab"), "Zoomed Reader pages must expose the drag hand cursor.");
assert.ok(css.includes('data-panning="true"') && css.includes("cursor: grabbing"), "Active Reader dragging must expose the closed hand cursor.");
assert.ok(css.includes("cursor: default"), "The baseline Reader page must retain the arrow cursor.");
assert.ok(css.includes(".newspaper-reader-control-section") && css.includes("border-left: 1px solid var(--border-soft)"), "Reader option groups need vertical dividers.");
assert.ok(css.includes(".newspaper-library-virtual") && css.includes(".newspaper-reader-virtual"), "Library and reader need virtual scroll geometry.");
assert.ok(!reader.includes("newspaper-reader-backdrop"), "Reader must not be embedded inside a modal card.");
assert.ok(!view.includes("newspaper-page-header"), "Downloader must not spend vertical space on a duplicate page header.");
assert.ok(!view.includes("newspaper-panel-heading"), "Dispatch panels must not render title rows.");
assert.ok(!view.includes("newspaper-panel-step"), "Dispatch panels must not render numbered steps.");
assert.ok(!view.includes("newspaper-download-now"), "Newspaper download action must reuse the LinkedIn primary button without custom visual overrides.");
assert.ok(!view.includes("newspaper-add-schedule"), "Schedule action must reuse the shared compact button without custom size overrides.");
assert.ok(!view.includes("newspaper-progress-title"), "Progress table must not be nested inside a titled outer card.");
assert.ok(!view.includes("<small>{compressionLabel}"), "Optimization guidance must not reserve layout height.");
assert.ok(css.includes("contain: layout paint"), "Resize-heavy newspaper surfaces must contain layout and paint invalidation.");
assert.ok(css.includes(':root[data-window-resizing="true"]'), "Rapid window resizing must temporarily disable expensive visual transitions.");
assert.ok(!view.includes("<h1"), "Downloader should keep the LinkVault shell as the application-level heading.");

console.log("UI contract verification passed.");
