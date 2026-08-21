import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

async function read(relativePath) {
  return readFile(path.join(desktop, relativePath), "utf8");
}

const [app, view, ipc, types, styles, packageText, rootPackageText] = await Promise.all([
  read("src/App.tsx"),
  read("src/components/youtube/YouTubeView.tsx"),
  read("src/lib/youtube/ipc.ts"),
  read("src/lib/youtube/types.ts"),
  read("src/index.css"),
  read("package.json"),
  read("../../package.json")
]);

const packageJson = JSON.parse(packageText);
const rootPackageJson = JSON.parse(rootPackageText);

function includes(source, fragment, message) {
  assert.ok(source.includes(fragment), message ?? `Missing ${fragment}`);
}

function matches(source, expression, message) {
  assert.match(source, expression, message);
}

// The route must remain a mounted application view, not an orphaned component.
includes(app, 'type AppView = "downloads"', "AppView does not define the existing route union");
includes(app, '"youtube"', "AppView does not include the YouTube route");
includes(app, 'aria-label="Open YouTube archive"', "YouTube navigation item is not discoverable");
includes(app, 'activeView === "youtube" ? (', "YouTube route is not mounted in the main view switch");
includes(app, "<YouTubeView />", "YouTube route does not render YouTubeView");

// IPC stays behind the typed adapter and must use the event-first reconciliation contract.
for (const command of [
  '"scan_youtube_source"',
  '"start_youtube_download"',
  '"get_youtube_download_state"',
  '"cancel_youtube_download"',
  '"get_youtube_helper_status"'
]) includes(ipc, command, `YouTube IPC adapter omits ${command}`);
matches(ipc, /invoke<ScanYouTubeSourceResponse>\("scan_youtube_source"/, "Source scan IPC is not typed");
matches(ipc, /invoke<StartYouTubeDownloadResponse>\("start_youtube_download"/, "Download start IPC is not typed");
matches(ipc, /invoke<GetYouTubeHelperStatusResponse>\("get_youtube_helper_status"/, "Helper status IPC is not typed");
matches(ipc, /return \{ status: "ready", code: null, message: "" \}/, "Browser preview does not remain helper-ready");
matches(ipc, /listen<YouTubeProgressEvent>\(/, "Run-change listener is not typed");
matches(view, /subscribeYouTubeRunChanged/, "YouTube view does not subscribe to run events");
matches(
  view,
  /const cleanup = await subscribeYouTubeRunChanged\([\s\S]*?const snapshot = await getYouTubeDownloadState\(\{ runId: null \}\)/,
  "YouTube view must subscribe before reconciling the initial run state"
);

// Polling is prohibited. Preview fixture timers are intentionally bounded and event-driven.
assert.doesNotMatch(view, /\bsetInterval\s*\(/, "YouTube view introduced polling");
assert.doesNotMatch(ipc, /\bsetInterval\s*\(/, "YouTube IPC adapter introduced polling");
matches(ipc, /window\.dispatchEvent\(new CustomEvent<YouTubeProgressEvent>/, "Preview progress does not use the typed event shape");
matches(view, /latestRevisionRef\.current/, "Run reconciliation does not guard against stale revisions");
matches(view, /snapshot\.revision <= latestRevisionRef\.current/, "Run reconciliation accepts stale revisions");

// Typed source/reel selection is occurrence-based, including playlist duplicates.
for (const type of [
  "ScanYouTubeSourceRequest",
  "ScanYouTubeSourceResponse",
  "StartYouTubeDownloadRequest",
  "StartYouTubeDownloadResponse",
  "GetYouTubeHelperStatusResponse",
  "YouTubeScanItem",
  "YouTubeProgressEvent",
  "YouTubeRunSnapshot"
]) matches(types, new RegExp(`(?:interface|type) ${type}\\b`), `Missing typed YouTube contract ${type}`);
matches(types, /type YouTubeHelperBackendStatus = "ready" \| "blocked"/, "Helper status response does not use the runtime ready/blocked contract");
matches(types, /interface GetYouTubeHelperStatusResponse[\s\S]*status: YouTubeHelperBackendStatus[\s\S]*code: string \| null[\s\S]*message: string/, "Helper status response shape is not typed");
matches(view, /selectedOccurrenceIds[^\n]*useState<Set<string>>/, "Selection is not keyed by occurrence ID");
matches(view, /useState<HelperStatus>\(nativeRuntime \? "pending" : "ready"\)/, "Helper status does not fail closed natively or remain ready in preview");
matches(view, /getYouTubeHelperStatus\(\)/, "Native YouTube view does not load helper status");
matches(view, /response\.status === "ready"/, "Helper status response is not mapped to the native readiness gate");
matches(view, /youtube-helper-error[\s\S]*?role="alert"/, "Helper failures are not exposed through an accessible alert");
matches(view, /new Set\(available\.map\(\(item\) => item\.occurrenceId\)\)/, "Select-all does not preserve occurrence identity");
matches(view, /item\.availability === "unavailable"/, "Unavailable occurrences are not represented");
matches(view, /disabled=\{unavailable \|\| activeRun\}/, "Unavailable occurrences remain selectable");
matches(view, /aria-label=\{`Select occurrence \$\{item\.ordinal\}: \$\{item\.title\}`\}/, "Occurrence selection lacks an accessible name");
matches(ipc, /playlistMode === "playlist" \|\| parsed\.playlistId !== null/, "Playlist URL handling is not explicit");

// Internal-only owner-risk acceptance and the restricted-content guardrails must remain visible.
for (const phrase of [
  "Internal-use guardrail",
  "public videos and playlists you own or are authorized to save",
  "private",
  "member-only",
  "paid",
  "age-gated",
  "Cookies",
  "DRM/access-control bypass",
  "public distribution"
]) includes(view, phrase, `YouTube guardrail copy omits ${phrase}`);
matches(view, /acknowledged.*localStorage|localStorage.*acknowledgement/s, "Owner-risk acknowledgement is not persisted locally");

// The route exposes a usable keyboard/screen-reader surface and a live progress channel.
for (const fragment of [
  'role="note"',
  'role="status"',
  'role="alert"',
  'aria-live="polite"',
  'aria-label="Public YouTube URL"',
  'aria-label="YouTube output directory"',
  'aria-label="Scanned YouTube occurrences"'
]) includes(view, fragment, `YouTube view is missing ${fragment}`);
matches(view, /onKeyDown=\{\(event\) => \{[\s\S]*?event\.key === "Enter"/, "Source scan cannot be keyboard submitted");
matches(view, /disabled=\{!scan \|\| selectedCount === 0/, "Start action is not gated on a selected occurrence");
matches(view, /disabled=\{!activeRun\}/, "Cancel action is not gated on an active run");

// Layout contracts are container-based and honor reduced-motion preferences.
for (const fragment of [
  '.lv-content[data-active-view="youtube"]',
  "@container lv-main (max-width: 980px)",
  "@container lv-main (max-width: 720px)",
  "@container lv-main (max-width: 520px)",
  "@media (prefers-reduced-motion: reduce)",
  ".youtube-view *::before",
  "min-width: 0"
]) includes(styles, fragment, `YouTube layout is missing ${fragment}`);

assert.equal(packageJson.scripts["verify:youtube-ui"], "node ./scripts/verify-youtube-ui.mjs", "Desktop YouTube UI verifier is not wired");
assert.equal(rootPackageJson.scripts["verify:youtube-ui"], "npm --prefix apps/desktop run verify:youtube-ui", "Root YouTube UI verifier is not wired");

console.log("YouTube UI route, typed IPC, event reconciliation, guardrails, accessibility, selection, and responsive contracts passed.");
