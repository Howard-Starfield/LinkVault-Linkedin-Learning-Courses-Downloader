import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

async function read(relativePath) {
  return readFile(path.join(desktop, relativePath), "utf8");
}

const [app, view, ipc, types, detect, preferences, styles, packageText, rootPackageText] = await Promise.all([
  read("src/App.tsx"),
  read("src/components/youtube/YouTubeView.tsx"),
  read("src/lib/youtube/ipc.ts"),
  read("src/lib/youtube/types.ts"),
  read("src/lib/youtube/detect.ts"),
  read("src/lib/youtube/preferences.ts"),
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
  '"inspect_youtube_transcripts"',
  '"pause_youtube_download"',
  '"resume_youtube_download"',
  '"get_youtube_helper_status"',
  '"get_youtube_preferences"',
  '"save_youtube_preferences"'
]) includes(ipc, command, `YouTube IPC adapter omits ${command}`);
matches(ipc, /invoke<ScanYouTubeSourceResponse>\("scan_youtube_source"/, "Source scan IPC is not typed");
matches(ipc, /invoke<StartYouTubeDownloadResponse>\("start_youtube_download"/, "Download start IPC is not typed");
matches(ipc, /invoke<GetYouTubeHelperStatusResponse>\("get_youtube_helper_status"/, "Helper status IPC is not typed");
matches(ipc, /invoke<SavedYouTubePreferences>\("get_youtube_preferences"/, "Preferences get IPC is not typed");
matches(ipc, /invoke<SavedYouTubePreferences>\("save_youtube_preferences"/, "Preferences save IPC is not typed");
matches(ipc, /invoke<InspectYouTubeTranscriptsResponse>\("inspect_youtube_transcripts"/, "Transcript inspection IPC is not typed");
matches(ipc, /invoke<YouTubeRunSnapshot>\("pause_youtube_download"/, "Pause IPC is not typed");
matches(ipc, /invoke<YouTubeRunSnapshot>\("resume_youtube_download"/, "Resume IPC is not typed");
matches(ipc, /return \{ status: "ready", code: null, message: "" \}/, "Browser preview does not remain helper-ready");
matches(ipc, /listen<YouTubeProgressEvent>\(/, "Run-change listener is not typed");
matches(view, /subscribeYouTubeRunChanged/, "YouTube view does not subscribe to run events");
matches(
  view,
  /const cleanup = await subscribeYouTubeRunChanged\([\s\S]*?const snapshot = await getYouTubeDownloadState\(\{ runId: null \}\)/,
  "YouTube view must subscribe before reconciling the initial run state"
);
matches(view, /loadYouTubeOutputDir\(\)/, "YouTube view does not load persisted output directory on mount");
matches(view, /persistYouTubeOutputDir\(/, "YouTube view does not save output directory through the preferences helper");
assert.doesNotMatch(view, /writeSavedYouTubeOutputDir|readSavedYouTubeOutputDir/, "YouTube view still uses legacy localStorage preference helpers");
assert.doesNotMatch(view, /localStorage/, "YouTube view still touches localStorage directly");
matches(preferences, /export async function loadYouTubeOutputDir/, "Preferences helper does not export loadYouTubeOutputDir");
matches(preferences, /export async function persistYouTubeOutputDir/, "Preferences helper does not export persistYouTubeOutputDir");
matches(preferences, /saveYouTubePreferences\(\{ output_dir: legacy \}\)/, "Legacy localStorage migrate does not save through IPC");
matches(preferences, /removeStorage\(LEGACY_OUTPUT_DIR_KEY\)/, "Legacy localStorage is not removed after a successful migrate");
matches(preferences, /writePreviewYouTubeOutputDir/, "Preview preference write helper is missing");
matches(preferences, /const saved = await saveYouTubePreferences\(\{ output_dir: trimmed \}\)/, "Tauri persist does not save via IPC alone");
assert.doesNotMatch(
  preferences,
  /saveYouTubePreferences\([\s\S]{0,200}?writeStorage|saveYouTubePreferences\([\s\S]{0,200}?localStorage\.setItem/,
  "Production preference save must not dual-write localStorage"
);
matches(types, /interface SavedYouTubePreferences[\s\S]*output_dir: string/, "SavedYouTubePreferences contract is missing snake_case output_dir");
assert.doesNotMatch(app, /saveYouTubePreferences|getYouTubePreferences|loadYouTubeOutputDir/, "YouTube preferences must not appear in the global Settings dialog");

// Polling is prohibited. Preview fixture timers are intentionally bounded and event-driven.
assert.doesNotMatch(view, /\bsetInterval\s*\(/, "YouTube view introduced polling");
assert.doesNotMatch(ipc, /\bsetInterval\s*\(/, "YouTube IPC adapter introduced polling");
matches(ipc, /window\.dispatchEvent\(new CustomEvent<YouTubeProgressEvent>/, "Preview progress does not use the typed event shape");
matches(view, /latestRevisionRef\.current/, "Run reconciliation does not guard against stale revisions");
matches(view, /snapshot\.revision <= latestRevisionRef\.current/, "Run reconciliation accepts stale revisions");
matches(view, /currentRunId !== null && !sameRun && !allowRunSwitch/, "Run reconciliation accepts an event-driven snapshot from a different run");
matches(view, /currentRunId !== null && currentRunId !== event\.runId/, "Run events are not scoped to the accepted run");
matches(view, /applyRunSnapshot\(snapshot, true\)/, "Authoritative reconciliation cannot explicitly switch runs");
matches(view, /latestRunIdRef\.current = response\.receipt\.runId[\s\S]*?getYouTubeDownloadState\(\{ runId: response\.receipt\.runId \}\)/, "Start does not claim successor-run events before state reconciliation");

// Typed source/reel selection is occurrence-based, including playlist duplicates.
for (const type of [
  "ScanYouTubeSourceRequest",
  "ScanYouTubeSourceResponse",
  "StartYouTubeDownloadRequest",
  "StartYouTubeDownloadResponse",
  "InspectYouTubeTranscriptsRequest",
  "InspectYouTubeTranscriptsResponse",
  "YouTubeTranscriptTrack",
  "MutateYouTubeRunRequest",
  "GetYouTubeHelperStatusResponse",
  "SavedYouTubePreferences",
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
matches(view, /item\.availability === "available"/, "Available occurrences are not represented explicitly");
matches(view, /disabled=\{!available \|\| activeRun/, "Non-public or unconfirmed occurrences remain selectable");
matches(view, /item\.availability === "unknown" \? "Unconfirmed" : "Unavailable"/, "Unknown availability is not distinguished from confirmed public content");
matches(view, /aria-label=\{`Download occurrence \$\{video\.item\.ordinal\}: \$\{video\.item\.title\}`\}/, "Occurrence download lacks an accessible name");
matches(ipc, /const playlist = request\.playlistMode === "playlist"/, "Playlist URL handling is not explicit");
matches(view, /playlistMode[\s\S]*scanYouTubeSource\([\s\S]*?playlistMode/, "Ambiguous video+playlist URLs do not send an explicit playlist mode");
matches(view, /aria-label="When URL includes a video and playlist"/, "Ambiguous video+playlist URLs do not expose a choice");
matches(view, /preferredLanguage[\s\S]*startYouTubeDownload\([\s\S]*?preferredLanguage/, "Preferred transcript language is not sent from typed state");
matches(view, /fallbackLanguages[\s\S]*startYouTubeDownload\([\s\S]*?fallbackLanguages/, "Fallback transcript languages are not sent from typed state");
matches(view, /aria-label="Preferred transcript language"/, "Preferred transcript language control is missing");
matches(view, /refreshDetectedLanguages|inspectYouTubeTranscripts/, "Preferred language is not detected from inspected transcript tracks");
matches(view, /collectLanguageOptions/, "Detected caption languages are not collected into dropdown options");
assert.doesNotMatch(view, /aria-label="Fallback transcript languages"/, "Fallback language is still a free-text field");
assert.doesNotMatch(view, /aria-label="Allow automatic captions"|Auto captions/, "Automatic-caption checkbox is still mounted");
assert.doesNotMatch(view, /Continue without transcript|Continue if transcript is missing/, "Continue-without-transcript checkbox is still mounted");
assert.doesNotMatch(view, /preferredLanguage:\s*null/, "Download request hard-codes a null preferred language");
assert.doesNotMatch(view, /fallbackLanguages:\s*\[\]/, "Download request hard-codes an empty fallback list");
matches(view, /allowAutomaticCaptions:\s*true/, "Automatic captions are not kept as a hidden default");
matches(view, /continueWithoutTranscript:\s*true/, "Continue-without-transcript is not kept as a hidden default");
matches(view, /useState<StartYouTubeDownloadRequest\["preferredLanguage"\]>\(null\)/, "Preferred language does not start unset until captions are inspected");
matches(view, /label: "No caption"/, "Missing captions are not shown as No caption");
assert.doesNotMatch(view, /\{ tag: "en", label: "English" \}/, "English is still hard-coded into language options before inspection");
matches(view, /onClick=\{\(\) => void pickOutputDirectory\(\)\}/, "Choosing a folder does not open the directory picker");
matches(ipc, /export async function startYouTubeUiMock/, "YouTube UI mock download is not exported");
matches(app, /Mock YouTube download/, "Settings does not expose a YouTube mock download");
matches(app, /startYouTubeUiMock\(\)/, "Settings mock does not start the YouTube UI mock run");

// Guardrail dialog, banner, acknowledgement storage, and CSS must stay gone.
assert.doesNotMatch(view, /Internal-use guardrail/, "YouTube view still renders internal-use guardrail copy");
assert.doesNotMatch(view, /ACKNOWLEDGEMENT_KEY|FIRST_USE_ACKNOWLEDGEMENT|PERSISTENT_GUIDANCE/, "YouTube view still keeps guardrail acknowledgement constants");
assert.doesNotMatch(view, /guardrailOpen|youtube-guardrail|rememberGuardrailChoice/, "YouTube view still mounts a guardrail dialog");
assert.doesNotMatch(view, /localStorage/, "YouTube view still persists a guardrail acknowledgement");
assert.doesNotMatch(styles, /youtube-guardrail|youtube-guidance/, "YouTube CSS still includes guardrail layout");
matches(view, /folderGateOpen/, "Destination-folder gate dialog state is missing");
matches(view, /Choose destination folder/, "Destination-folder gate does not expose a folder picker action");
matches(view, /onPointerDown=\{handleSourcePointerDown\}/, "URL field does not intercept clicks when no destination folder is set");
matches(view, /ensureDestinationFolder/, "URL interactions do not check for a destination folder");
matches(view, /disabled=\{activeRun\}/, "URL field remains helper-blocked instead of clickable until a run is active");
matches(styles, /\.youtube-folder-gate-actions/, "Destination-folder gate dialog styles are missing");

// Search-first paste surface: detect complete YouTube URLs and scan without a required extra click.
matches(view, /from "\.\.\/\.\.\/lib\/youtube\/detect"/, "YouTube view does not use the typed URL detector");
matches(detect, /export function detectYouTubeLinks/, "YouTube URL detector is not exported");
matches(detect, /youtube\.com/, "YouTube URL detector does not recognize youtube.com");
matches(detect, /youtu\.be/, "YouTube URL detector does not recognize youtu.be");
matches(detect, /complete:/, "YouTube URL detector does not mark complete links");
matches(view, /onPaste=\{handlePaste\}/, "Paste does not auto-detect YouTube links");
matches(view, /handlePaste\([\s\S]*?detectYouTubeLinks\([\s\S]*?requestAutoScan\(links\)/, "Complete pasted URLs do not auto-scan");
matches(view, /handleSourceChange\([\s\S]*?scheduleAutoScan\(links\)/, "Typed URL changes do not debounce auto-scan");
matches(view, /YOUTUBE_AUTO_SCAN_DEBOUNCE_MS/, "Typed URL auto-scan debounce constant is missing");
matches(view, /YOUTUBE_LANGUAGE_PROBE_LIMIT/, "Caption language probe is not capped after scan");
matches(view, /refreshDetectedLanguages\([\s\S]*?YOUTUBE_LANGUAGE_PROBE_LIMIT/, "Post-scan caption inspection is not bounded");
matches(view, /languageProbeTokenRef/, "Language probe supersession token is missing");
matches(view, /probeToken === languageProbeTokenRef\.current/, "Superseded language probes can leave Language permanently disabled");
matches(view, /setIsDetectingLanguages\(false\)/, "Rescan/reset does not clear language detection busy state");
matches(view, /isYouTubeRunSnapshot\(event\)/, "Full run snapshots from events are refetched instead of applied");
matches(view, /ensureLanguageOptionsForSelection/, "Language picker does not lazily probe captions when still empty");
matches(view, /handleSearchKeyDown\([\s\S]*?autoScanTimerRef[\s\S]*?handleScan/, "Enter-to-scan does not cancel a pending typed auto-scan debounce");
matches(view, /detectedKindLabel\(kind\)/, "Detected links are not presented as video vs playlist results");
matches(view, /youtube-search-stage/, "Search-first stage is missing");
matches(view, /youtube-control-cluster/, "Download options are not unified into a compact control cluster");
matches(view, /useState<YouTubeDownloadMode>\("video_and_transcript"\)/, "Capture mode does not default to video+transcript");
assert.doesNotMatch(view, /aria-label="Search YouTube URL"|Search YouTube URL/, "Search button is still mounted");
assert.doesNotMatch(view, /Helper gate ready|Browser preview uses deterministic fixture data/, "Helper/preview chrome is still visible");
assert.doesNotMatch(view, /<h2>YouTube<\/h2>/, "YouTube page heading is still mounted");
assert.doesNotMatch(view, /<h3>Download<\/h3>|youtube-run-panel/, "Standalone download card is still mounted");
assert.doesNotMatch(view, /Preview YouTube source|Choose folder & start|Progress is event-driven/, "Preview/queue chrome is still mounted");
assert.doesNotMatch(view, /youtube-search-hint|Paste a complete youtube\.com/, "Paste hint under the search box is still mounted");
matches(view, /Download all/, "Multiple results do not expose Download all");
matches(view, /youtube-download-overlay-button/, "Download is not overlaid on the result row");
matches(view, /youtube-result-progress/, "Result rows do not show a live progress bar");
matches(view, /syncSearchInputHeight|YOUTUBE_SEARCH_MAX_HEIGHT_PX/, "Search box does not auto-expand before the height cap");
matches(view, /sourceNewlineCount|lastHeightSyncKeyRef/, "Search box height sync still runs on every keystroke");
matches(view, /function YouTubeScanSkeletonRows/, "Scan skeleton row helper is missing");
matches(view, /youtube-result-row-skeleton/, "Scan skeleton rows are missing the skeleton class");
matches(view, /YOUTUBE_SCAN_SKELETON_COUNT/, "Empty-scan skeleton count constant is missing");
matches(view, /isScanning && videos\.length === 0 \? \([\s\S]*?<YouTubeScanSkeletonRows count=\{YOUTUBE_SCAN_SKELETON_COUNT\} \/>/, "Empty scan does not show primary skeleton placeholders");
matches(view, /isScanning && videos\.length > 0 \? \([\s\S]*?<YouTubeScanSkeletonRows count=\{1\} \/>/, "Multi-link scan does not keep results with a trailing skeleton");
matches(view, /isScanning \? \([\s\S]*?Finding videos…/, "Scan status copy is missing while skeletons are shown");
matches(styles, /\.youtube-result-row-skeleton/, "YouTube scan skeleton styles are missing");
matches(styles, /\.youtube-skeleton-line/, "YouTube skeleton line styles are missing");
matches(styles, /@keyframes youtube-skeleton-shimmer/, "YouTube skeleton shimmer animation is missing");
matches(styles, /\.youtube-result-row \{[\s\S]*?border-radius: 10px;/, "Result rows do not use the shared 10px radius");
includes(styles, ".youtube-view *::before", "YouTube reduced-motion contract no longer targets pseudo-elements");

// The route exposes a usable keyboard/screen-reader surface and a live progress channel.
for (const fragment of [
  'role="status"',
  'role="alert"',
  'aria-live="polite"',
  'aria-label="Public YouTube URL"',
  'aria-label="YouTube output directory"',
  'aria-label="Scanned YouTube occurrences"',
  'aria-label="Detected YouTube links"'
]) includes(view, fragment, `YouTube view is missing ${fragment}`);
matches(view, /onKeyDown=\{\(event\) => \{[\s\S]*?event\.key === "Enter"|function handleSearchKeyDown\([\s\S]*?event\.key === "Enter"/, "Source scan cannot be keyboard submitted");
matches(view, /disabled=\{availableVideos\.length === 0/, "Download all is not gated on a detected occurrence");
assert.doesNotMatch(view, /disabled=\{[^}]*!outputDir\.trim\(\)/, "An empty output directory still greys the Start action instead of opening the folder picker");
matches(view, /outputDir\.trim\(\) \|\| await pickOutputDirectory\(\)/, "Start does not request an output directory when one has not been chosen");
matches(view, /mode !== "video_only"[\s\S]*?requestTranscriptInspection\(/, "Transcript modes do not satisfy native inspection admission before start");
matches(view, /disabled=\{!activeRun\}/, "Cancel action is not gated on an active run");
matches(view, /inspectYouTubeTranscripts\(/, "Transcript inspection is not mounted through the typed adapter");
matches(view, /transcriptInspectionGenerationRef\.current/, "Transcript inspection does not correlate responses with the active selection");
matches(view, /requestGeneration !== transcriptInspectionGenerationRef\.current/, "Stale transcript inspection responses are accepted");
matches(view, /response\.occurrences\.every\(\(occurrence, index\) => occurrence\.occurrenceId === requestedOccurrenceIds\[index\]\)/, "Transcript response identities are not checked against the request");
matches(view, /scanYouTubeSource\([\s\S]*?transcriptInspectionGenerationRef\.current \+= 1;[\s\S]*?setScanPlans/, "A committed rescan does not invalidate overlapping transcript inspection");
matches(view, /expectedRevision: runSnapshot\.revision/, "Pause/resume actions are not revision-aware");
matches(view, /canResumeRun \? "Resume" : "Pause"/, "Pause/resume controls are not mounted on results");
matches(view, />\s*Cancel\s*</, "Cancel control is not mounted on results");

// Layout contracts are container-based and honor reduced-motion preferences.
for (const fragment of [
  '.lv-content[data-active-view="youtube"]',
  "@container lv-main (max-width: 980px)",
  "@container lv-main (max-width: 720px)",
  "@container lv-main (max-width: 520px)",
  ".youtube-search-stage",
  ".youtube-search-input",
  ".youtube-cluster-folder",
  ".youtube-folder-field",
  ".youtube-option-language",
  ".youtube-result-row",
  ".youtube-result-overlay",
  "@media (prefers-reduced-motion: reduce)",
  ".youtube-view *::before",
  "min-width: 0",
  "border-radius: 10px"
]) includes(styles, fragment, `YouTube layout is missing ${fragment}`);
matches(styles, /\.youtube-search-input \{[\s\S]*?border-radius: 10px;/, "Search box roundness does not match Save to");
matches(styles, /\.youtube-folder-field \{[\s\S]*?border-radius: 10px;/, "Save to folder field roundness drifted");
matches(styles, /\.youtube-control-cluster[\s\S]*?select \{[\s\S]*?border-radius: 10px;/, "Option dropdowns do not match Save to roundness");
matches(styles, /\.youtube-search-input \{[\s\S]*?max-height: 132px;/, "Search box height cap is missing");
assert.doesNotMatch(styles, /\.youtube-search-hint/, "Search hint styles are still present");

assert.equal(packageJson.scripts["verify:youtube-ui"], "node ./scripts/verify-youtube-ui.mjs", "Desktop YouTube UI verifier is not wired");
assert.equal(rootPackageJson.scripts["verify:youtube-ui"], "npm --prefix apps/desktop run verify:youtube-ui", "Root YouTube UI verifier is not wired");

console.log("YouTube UI route, typed IPC, event reconciliation, search-first paste, accessibility, selection, and responsive contracts passed.");
