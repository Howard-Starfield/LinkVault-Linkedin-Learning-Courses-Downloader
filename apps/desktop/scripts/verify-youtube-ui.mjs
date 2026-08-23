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
  '"inspect_youtube_transcripts"',
  '"pause_youtube_download"',
  '"resume_youtube_download"',
  '"get_youtube_helper_status"'
]) includes(ipc, command, `YouTube IPC adapter omits ${command}`);
matches(ipc, /invoke<ScanYouTubeSourceResponse>\("scan_youtube_source"/, "Source scan IPC is not typed");
matches(ipc, /invoke<StartYouTubeDownloadResponse>\("start_youtube_download"/, "Download start IPC is not typed");
matches(ipc, /invoke<GetYouTubeHelperStatusResponse>\("get_youtube_helper_status"/, "Helper status IPC is not typed");
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

// Polling is prohibited. Preview fixture timers are intentionally bounded and event-driven.
assert.doesNotMatch(view, /\bsetInterval\s*\(/, "YouTube view introduced polling");
assert.doesNotMatch(ipc, /\bsetInterval\s*\(/, "YouTube IPC adapter introduced polling");
matches(ipc, /window\.dispatchEvent\(new CustomEvent<YouTubeProgressEvent>/, "Preview progress does not use the typed event shape");
matches(view, /latestRevisionRef\.current/, "Run reconciliation does not guard against stale revisions");
matches(view, /snapshot\.revision <= latestRevisionRef\.current/, "Run reconciliation accepts stale revisions");
matches(view, /currentRunId !== null && !sameRun && !allowRunSwitch/, "Run reconciliation accepts an event-driven snapshot from a different run");
matches(view, /currentRunId !== null && currentRunId !== event\.runId/, "Run events are not scoped to the accepted run");
matches(view, /applyRunSnapshot\(snapshot, true\)/, "Authoritative reconciliation cannot explicitly switch runs");
matches(view, /latestRunIdRef\.current = response\.runId[\s\S]*?getYouTubeDownloadState\(\{ runId: response\.runId \}\)/, "Start does not claim successor-run events before state reconciliation");

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
matches(view, /disabled=\{!available \|\| activeRun\}/, "Non-public or unconfirmed occurrences remain selectable");
matches(view, /item\.availability === "unknown" \? "Unconfirmed" : "Unavailable"/, "Unknown availability is not distinguished from confirmed public content");
matches(view, /aria-label=\{`Select occurrence \$\{item\.ordinal\}: \$\{item\.title\}`\}/, "Occurrence selection lacks an accessible name");
matches(ipc, /const playlist = request\.playlistMode === "playlist"/, "Playlist URL handling is not explicit");
matches(view, /playlistMode[\s\S]*scanYouTubeSource\([\s\S]*?playlistMode/, "Ambiguous video+playlist URLs do not send an explicit playlist mode");
matches(view, /aria-label="When URL includes a video and playlist"/, "Ambiguous video+playlist URLs do not expose a choice");
matches(view, /preferredLanguage[\s\S]*startYouTubeDownload\([\s\S]*?preferredLanguage/, "Preferred transcript language is not sent from typed state");
matches(view, /fallbackLanguages[\s\S]*startYouTubeDownload\([\s\S]*?fallbackLanguages/, "Fallback transcript languages are not sent from typed state");
matches(view, /aria-label="Preferred transcript language"/, "Preferred transcript language control is missing");
matches(view, /aria-label="Fallback transcript languages"/, "Fallback transcript language control is missing");
matches(view, /aria-label="Allow automatic captions"/, "Automatic-caption choice is not explicitly labelled");
assert.doesNotMatch(view, /preferredLanguage:\s*null/, "Download request hard-codes a null preferred language");
assert.doesNotMatch(view, /fallbackLanguages:\s*\[\]/, "Download request hard-codes an empty fallback list");

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
matches(view, /<Dialog[\s\S]*?open=\{guardrailOpen\}[\s\S]*?title="YouTube Internal-use guardrail"/, "First-use guardrail is not mounted as an accessible dialog");
matches(view, /description=\{FIRST_USE_ACKNOWLEDGEMENT\}/, "Guardrail dialog does not expose the public-content acknowledgement copy");
matches(view, /aria-label="Don't show this again"/, "Guardrail dialog does not expose a remember-choice checkbox");
matches(view, /Continue to YouTube archive/, "Guardrail dialog does not expose a clear continuation action");
matches(view, /if \(rememberGuardrailChoiceRef\.current\) window\.localStorage\.setItem\(ACKNOWLEDGEMENT_KEY, "true"\)/, "Guardrail acknowledgement is persisted without checking the remember choice");
matches(view, /else window\.localStorage\.removeItem\(ACKNOWLEDGEMENT_KEY\)/, "Guardrail remember choice cannot be cleared");
matches(view, /setAcknowledged\(true\)[\s\S]*?setGuardrailOpen\(false\)/, "Closing the guardrail does not clear the current-session gate");
assert.doesNotMatch(view, /disabled=\{[^}]*acknowledged/, "Scan/start controls remain disabled by the acknowledgement state");
assert.doesNotMatch(view, /if \(!acknowledged\)/, "Scan/start handlers still require a persistent acknowledgement gate");

// The route exposes a usable keyboard/screen-reader surface and a live progress channel.
for (const fragment of [
  'role="note"',
  'role="status"',
  'role="alert"',
  'aria-live="polite"',
  'Review guardrail',
  'aria-label="Public YouTube URL"',
  'aria-label="YouTube output directory"',
  'aria-label="Scanned YouTube occurrences"'
]) includes(view, fragment, `YouTube view is missing ${fragment}`);
matches(view, /onKeyDown=\{\(event\) => \{[\s\S]*?event\.key === "Enter"/, "Source scan cannot be keyboard submitted");
matches(view, /disabled=\{!scan \|\| selectedCount === 0/, "Start action is not gated on a selected occurrence");
assert.doesNotMatch(view, /disabled=\{[^}]*!outputDir\.trim\(\)/, "An empty output directory still greys the Start action instead of opening the folder picker");
matches(view, /outputDir\.trim\(\) \|\| await pickOutputDirectory\(\)/, "Start does not request an output directory when one has not been chosen");
matches(view, /mode !== "video_only" && !transcriptInspection[\s\S]*?requestTranscriptInspection\(false\)/, "Transcript modes do not satisfy native inspection admission before start");
matches(view, /disabled=\{!activeRun\}/, "Cancel action is not gated on an active run");
matches(view, /inspectYouTubeTranscripts\(/, "Transcript inspection is not mounted through the typed adapter");
matches(view, /transcriptInspectionGenerationRef\.current/, "Transcript inspection does not correlate responses with the active selection");
matches(view, /requestGeneration !== transcriptInspectionGenerationRef\.current/, "Stale transcript inspection responses are accepted");
matches(view, /response\.occurrences\.every\(\(occurrence, index\) => occurrence\.occurrenceId === requestedOccurrenceIds\[index\]\)/, "Transcript response identities are not checked against the request");
matches(view, /scanYouTubeSource\([\s\S]*?transcriptInspectionGenerationRef\.current \+= 1;[\s\S]*?setScan\(nextScan\)/, "A committed rescan does not invalidate overlapping transcript inspection");
matches(view, /aria-label="Transcript inspection"/, "Transcript inspection lacks an accessible region");
matches(view, /expectedRevision: runSnapshot\.revision/, "Pause/resume actions are not revision-aware");
matches(view, /Pause after current/, "Pause-after-current control is not mounted");
matches(view, /Resume run/, "Resume control is not mounted");

// Layout contracts are container-based and honor reduced-motion preferences.
for (const fragment of [
  '.lv-content[data-active-view="youtube"]',
  "@container lv-main (max-width: 980px)",
  "@container lv-main (max-width: 720px)",
  "@container lv-main (max-width: 520px)",
  ".youtube-guardrail-dialog",
  ".youtube-guardrail-check",
  "@media (prefers-reduced-motion: reduce)",
  ".youtube-view *::before",
  "min-width: 0"
]) includes(styles, fragment, `YouTube layout is missing ${fragment}`);

assert.equal(packageJson.scripts["verify:youtube-ui"], "node ./scripts/verify-youtube-ui.mjs", "Desktop YouTube UI verifier is not wired");
assert.equal(rootPackageJson.scripts["verify:youtube-ui"], "npm --prefix apps/desktop run verify:youtube-ui", "Root YouTube UI verifier is not wired");

console.log("YouTube UI route, typed IPC, event reconciliation, guardrails, accessibility, selection, and responsive contracts passed.");
