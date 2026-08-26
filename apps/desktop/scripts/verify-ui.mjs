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
  "ensureNewspaperQueueProcessing",
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
  "pausingForReset",
  "queue-detail-overlay",
  "queue-job-stack",
  "createDownloadEmulatorJob",
  "Excel: Pivot tables for analysts",
  "CSS: Grid and flexbox layouts",
  "Leadership: Coaching your team",
  "UPDATE_TOAST_ID",
  "Install now"
]) {
  assert.ok(app.includes(required), `App shell is missing: ${required}`);
}

assert.ok(!app.includes('settings-section-title">Artifacts'), "Settings must not duplicate LinkedIn artifact toggles.");
assert.ok(!app.includes("Download videos by default"), "Artifact download defaults belong on the LinkedIn command board, not Settings.");

const main = await readFile(new URL("../src/main.tsx", import.meta.url), "utf8");
assert.ok(main.includes('position="bottom-left"'), "Sonner toasts must appear on the left.");

for (const required of [
  "queueNeedsSessionRefresh",
  "isLinkedInSessionError",
  "copyQueuedCourseUrl",
  "copyTextToClipboard",
  "onContextMenu",
  "Right-click to copy this course URL",
  "Resume queue"
]) {
  assert.ok(app.includes(required), `LinkedIn queue recovery is missing: ${required}`);
}

for (const required of [
  "Select newspaper editions",
  "Regional",
  "Weekly",
  "Special",
  "Add schedule",
  "scheduleDateModeLabel",
  "dateMode,",
  "System current date",
  "Delay",
  "Folder",
  "Quality",
  "compressionLabel",
  "92",
  "45",
  "35",
  "25",
  "Keep JPG",
  "JPG remains only if WebP is larger or fails",
  "Download",
  "newspaper-action-row",
  "newspaper-delay-unit",
  ">sec<",
  "Awaiting release",
  "Optimization worker mode",
  "Manual</option>",
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
  "NewspaperQueueSectionTab",
  "queue-section-tabs newspaper-queue-section-tabs",
  "No ${queueSection} editions",
  "Queued editions will appear here.",
  "newspaper-schedule-queue-row",
  "Does not block downloads",
  "Waits until scheduled time · does not block downloads",
  "Permanently delete this downloaded edition?",
  "local files and progress history were removed",
  "Pause this download before deleting it",
  'disabled={["active", "optimizing"].includes(job.status)}',
  'role="progressbar"',
]) {
  assert.ok(view.includes(required), `Newspaper view is missing: ${required}`);
}
assert.ok(!view.includes("92 - Math.round"), "Quality labels and submitted values must use the same explicit preset.");
assert.ok(
  !view.includes('invoke("process_newspaper_queue")'),
  "App owns the newspaper download queue worker; NewspaperView must request processing through onRequestQueueProcess."
);

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
assert.ok(app.includes('className="settings-dialog"'), "Settings must own a compact responsive dialog contract.");
assert.ok(!app.includes("lv-sidebar-optimization"), "Sidebar must not show newspaper optimization runtime chrome.");
assert.ok(css.includes(".lv-sidebar nav > *") && css.includes("min-width: 0"), "Sidebar nav children need an intrinsic-width owner.");
assert.ok(css.includes(".settings-dialog") && css.includes("overflow-x: hidden"), "Settings must not expose a horizontal scroll surface.");
assert.ok(css.includes(".queue-url-hint") && css.includes(".queue-session-warning"), "LinkedIn queue recovery needs copy and session-refresh affordances.");
assert.ok(!library.includes(">Read</Button>"), "Opening a newspaper must be owned by the whole library row.");
assert.ok(!library.includes("Register archive") && !library.includes("Repair existing"), "Archive maintenance belongs in Settings, not the Library toolbar.");
assert.ok(!library.includes("Default newspaper zoom"), "Reader defaults belong in Settings, not the Library toolbar.");
assert.ok(library.includes("item.readPageCount"), "Library progress must use unique viewed-page coverage.");
assert.ok(!library.includes("item.furthestPageIndex + 1"), "Library progress must not treat the furthest reached page as read coverage.");
assert.ok(!newspaperApi.includes("get_newspaper_preview"), "Thumbnail transport must not use the legacy base64 IPC command.");
assert.ok(!newspaperApi.includes("get_newspaper_page_image"), "Reader transport must not use the legacy base64 IPC command.");
assert.ok(css.includes(".newspaper-downloads-workspace"), "Newspaper download needs a LinkedIn-like centered workspace.");
assert.ok(css.includes(".newspaper-search-stage"), "Newspaper download needs a concise search/control stage.");
assert.ok(css.includes(".newspaper-control-cluster"), "Newspaper download needs a compact labeled control cluster.");
assert.ok(css.includes(".newspaper-queue-panel"), "Newspaper download needs a LinkedIn-like queue panel.");
assert.ok(css.includes("--newspaper-control-height: 32px"), "Newspaper controls must share the 32px control height.");
assert.ok(css.includes("border-radius: 10px"), "Newspaper download controls must use the shared 10px radius.");
assert.ok(css.includes(".newspaper-editions") && css.includes("width: min(100%, 420px)"), "Edition picker must stay narrower than the workspace.");
assert.ok(css.includes("minmax(90px, 140px)"), "Edition list height must stay compact (half of the prior 180–280 band).");
assert.ok(!view.includes("newspaper-schedule-panel"), "Separate Schedule/History panel must be removed.");
assert.ok(!view.includes("newspaper-schedule-section-tabs"), "Schedule/History section tabs must be removed.");
assert.ok(!view.includes("newspaper-schedule-tabs"), "Schedule/History must not keep underline-style newspaper schedule tabs.");
assert.ok(!view.includes("No schedules yet"), "Empty schedule placeholder must not reserve UI.");
assert.ok(!view.includes("newspaper-history-list"), "History must live in Completed/Failed queue tabs, not a separate panel.");
assert.ok(view.includes("newspaper-schedule-queue-row"), "Enabled and paused schedules must appear inside the Queue tab.");
assert.ok(css.includes(".newspaper-options-row") && css.includes("max-content"), "Optimize/Keep JPG column must size to content and avoid dead space.");
assert.ok(css.includes(".newspaper-control-cluster") && css.includes("margin-inline: auto 0"), "Control cluster must sit asymmetrically.");
assert.ok(!view.includes("Search editions"), "Edition search was removed to free vertical space.");
assert.ok(!view.includes("Delay between editions"), "Option labels must stay concise.");
assert.ok(!view.includes("Image compression strength"), "Compression label must stay concise.");
assert.ok(!view.includes("Keep source JPG"), "JPG retention label must stay concise.");
assert.ok(css.includes(".newspaper-progress-actions") && css.includes("opacity: 0"), "Queue actions must reveal on hover or focus.");
assert.ok(css.includes("@container lv-main (max-width: 900px)"), "Newspaper view needs the container-based responsive collapse.");
assert.ok(css.includes("@container lv-main (max-width: 650px)"), "Newspaper view needs the narrow newspaper stack.");
assert.ok(view.includes("newspaper-downloads-workspace"), "Newspaper download markup must use the concise workspace.");
assert.ok(view.includes("newspaper-search-stage"), "Newspaper download markup must use the search stage.");
assert.ok(view.includes("newspaper-control-cluster"), "Newspaper download markup must use the control cluster.");
assert.ok(view.includes("newspaper-queue-panel"), "Newspaper download markup must use the queue panel.");
assert.ok(view.includes("newspaper-queue-section-tabs"), "Newspaper queue must use Queue/Active/Completed/Failed section tabs.");
assert.ok(view.includes("newspaper-action-row"), "Folder, schedule, and download actions must share one row.");
assert.ok(!view.includes("Download now"), "Primary download action must use the shorter Download label.");
assert.ok(!view.includes("newspaper-dispatch-grid"), "Newspaper download must not keep the three-panel dispatch grid.");
assert.ok(!view.includes("newspaper-dispatch-panel"), "Newspaper download must not keep bordered dispatch panel chrome.");
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
assert.ok(css.includes("--queue-live:"), "LinkedIn queue live states must use a non-orange live color.");
assert.ok(css.includes(".queue-detail-overlay"), "LinkedIn video progress must overlay later queue rows.");
assert.ok(css.includes(".queue-job-stack.is-open"), "The expanded queue row must stack above later downloads.");
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
assert.ok(!view.includes("<h1"), "Downloader should keep the LinkedVault shell as the application-level heading.");

console.log("UI contract verification passed.");
