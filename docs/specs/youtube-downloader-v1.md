# PRD: YouTube Downloader V1

**Author:** LinkVault engineering  
**Date:** 2026-08-20  
**Status:** Internally authorized for Y0-Y3 implementation/testing under owner-risk acceptance; public distribution/release blocked pending Y-PUBLIC-REVIEW<br>
**Target:** LinkVault desktop / Windows-first  
**Provider owner:** `apps/desktop/src-tauri/src/providers/youtube/`  
**Related architecture:** [ADR-001: Unified workflow modular monolith](../architecture/adr-001-unified-workflow-modular-monolith.md) and [ADR-003: YouTube V1 transient workflow bridge](../architecture/adr-003-youtube-transient-workflow-bridge.md)

## 1. Summary

Add YouTube as a first-class LinkVault provider for downloading public YouTube videos and playlists together with uploader-provided or YouTube automatic transcripts.

V1 is deliberately narrow. It directly integrates bundled `yt-dlp`, Deno, FFmpeg and FFprobe helpers behind Rust-owned commands and the workflow-owned non-durable bridge accepted in ADR-003. It does **not** introduce a provider-local cancellation runtime, generic toolchain manager, independent binary updater, arbitrary yt-dlp command surface, browser-cookie authentication system, or a second durable workflow engine.

The core flow is:

```text
Paste URL
  -> scan video or playlist
  -> choose videos
  -> inspect transcript availability
  -> choose preferred/fallback languages and source preference
  -> choose quality
  -> download selected items sequentially
  -> merge media when required
  -> save raw VTT + normalized transcript JSON
```

The first release should be useful on a clean Windows installation without requiring the user to install Python, yt-dlp, Deno or FFmpeg separately.

## 2. Product goals

1. Make YouTube feel like a native LinkVault source rather than a command-line wrapper.
2. Support both individual videos and explicit playlists.
3. Scan and expose transcript availability before download.
4. Distinguish uploader-provided captions from automatic captions.
5. Allow preferred and fallback transcript-language selection.
6. Download media with simple quality caps rather than raw YouTube format IDs.
7. Save transcript artifacts in a form LinkVault can later search and render offline.
8. Keep V1 implementation small enough to ship and harden quickly.
9. Avoid architecture that would later block migration into LinkVault's shared workflow runtime.

## 3. Non-goals

V1 does not include:

- channel-wide downloads;
- YouTube search;
- YouTube Music-specific behavior;
- active livestream recording;
- upcoming-premiere waiting;
- comments or live chat archives;
- SponsorBlock;
- Whisper or local speech-to-text fallback;
- AI transcript cleanup or AI translation;
- chapter splitting;
- subtitle embedding into media containers;
- SRT export;
- more than one saved transcript language per video by default;
- browser-cookie authentication;
- member-only, private, age-gated or account-restricted access as a supported product promise;
- arbitrary yt-dlp arguments;
- yt-dlp plugins;
- an independent yt-dlp/FFmpeg updater;
- a generic app-wide toolchain-management subsystem;
- durable cross-restart scheduling or workflow leases.

## 4. Product principle: direct integration, not a toolchain platform

YouTube V1 requires external helper executables, but LinkVault should treat them as implementation dependencies of the YouTube provider rather than as a new user-facing toolchain system.

The Windows bundle should contain tested versions of:

```text
yt-dlp.exe
deno.exe
ffmpeg.exe
ffprobe.exe
```

Current yt-dlp YouTube support requires an external JavaScript runtime for full extraction; Deno is the recommended runtime. FFmpeg/FFprobe are required for reliable merging and media inspection.

V1 MUST NOT add:

```text
src/app/toolchain/
toolchain database tables
a generic component manager
a generic executable updater
user-configurable executable paths
```

Updates to bundled helpers ship through normal LinkVault releases.

### 4.1 Pinned helper lock and acquisition

External executables are release inputs, not ad-hoc repository binaries. The source of truth is `docs/third-party/youtube-helpers-lock.json`, with this minimum shape:

```json
{
  "schemaVersion": 1,
  "targetTriple": "x86_64-pc-windows-msvc",
  "lockDigest": "sha256-of-rfc8785-document-without-lockDigest",
  "components": [
    {
      "name": "yt-dlp",
      "version": "pinned-release",
      "filename": "yt-dlp-x86_64-pc-windows-msvc.exe",
      "sourceUrl": "pinned-https-url",
      "sourceArchiveUrl": "pinned-https-url",
      "sha256": "64-lowercase-hex-characters",
      "sizeBytes": 0,
      "sourceArchiveSha256": "64-lowercase-hex-characters",
      "sourceArchiveSizeBytes": 0,
      "archiveMember": null,
      "compatibility": {
        "ytDlpEjsVersion": "pinned-or-null",
        "ffmpegBuildId": "pinned-or-null"
      },
      "loadedAssets": [],
      "licenseId": "SPDX-expression",
      "licenseFile": "docs/third-party/youtube/..."
    }
  ]
}
```

The lock contains exactly one component for each required role (yt-dlp, Deno, FFmpeg and FFprobe), with executable-loaded companion assets nested under their owner; duplicate, missing or extra roles fail validation. It records exact asset/source sizes and hashes, extraction-member identity, yt-dlp/EJS composition compatibility, FFmpeg build configuration and corresponding-source obligations. `lockDigest` is SHA-256 over the UTF-8 RFC 8785 canonical JSON document with the `lockDigest` member omitted. No URL/version may use a floating `latest` asset.

`apps/desktop/scripts/fetch-youtube-helpers.mjs` downloads only lock-listed HTTPS assets into a temporary directory, checks asset and source size/SHA-256 plus exact extraction members before promotion, and emits the target-triple filenames required by Tauri. CI validates the lock schema, canonical digest, sources, notices and checksums before building. Before **every** YouTube helper launch, including scan and transcript inspection, the signed application verifies the executable and executable-loaded assets against the embedded lock; mismatch returns `HELPER_INTEGRITY_FAILED` before network access.

`THIRD_PARTY_NOTICES.md` and committed helper license/source materials are release-blocking inputs. The exact FFmpeg/FFprobe build, not the FFmpeg project in the abstract, determines the required notice and source-offer treatment.

## 5. Architecture fit and migration rule

[ADR-003](../architecture/adr-003-youtube-transient-workflow-bridge.md) authorizes a constrained, workflow-owned transient bridge until the Phase 2 durable kernel is ready.

`TransientWorkflowRuntime` owns:

- opaque run identity and monotonic revisions;
- the run transition matrix and terminal-state arbitration;
- pause, resume, cancellation and tracked shutdown;
- bounded progress delivery;
- Windows process-tree containment; and
- V1 external-process admission and resource permits.

The YouTube provider owns discovery, validation, immutable scan plans, yt-dlp argument construction, transcript normalization, item planning and artifact verification. It MUST NOT own a scheduler, cancellation registry, Tauri-managed global state or frontend processing loop.

The bridge is non-durable:

- no generic or provider-local durable job/event tables;
- no automatic retry scheduler;
- no cross-restart workflow guarantee;
- no background polling loop owned by React;
- no cross-provider imports; and
- no reuse by another provider without an ADR amendment.

V1 uses one atomic admission state: `Idle | Discovering(operationId) | Running(runId) | ShuttingDown`. Scan, inspection and start reserve it under one mutex before helper verification or network work, and matching RAII guards prevent stale/abandoned commands from releasing another operation. One runtime-global semaphore caps caption helpers at two. A run executes selected occurrences sequentially with one managed helper tree. Closing the window to the tray does not stop the run; true application exit enters `ShuttingDown`, cancels and joins every managed tree through the app-owned exit barrier. When the shared durable runtime is ready, the provider planner/executor adapters migrate without rewriting provider-domain logic or dual-writing state.

## 6. Proposed source layout

```text
apps/desktop/src-tauri/src/providers/youtube/
  mod.rs
  commands.rs
  models.rs
  urls.rs
  ytdlp.rs
  captions.rs
  thumbnails.rs
  paths.rs
  errors.rs

apps/desktop/src-tauri/src/workflow/transient/
  mod.rs
  runtime.rs
  transitions.rs
  admission.rs
  managed_process.rs

apps/desktop/src-tauri/src/app/
  cooperative_exit.rs       App-owned renderer/native participant barrier
  safe_output_filesystem.rs Validated roots, staging and publication

apps/desktop/src/components/youtube/
  YouTubeView.tsx

apps/desktop/src/lib/youtube/
  ipc.ts
  types.ts
```

Optional focused modules may be added when a file becomes too broad, but the provider MUST NOT import internal modules from LinkedIn, Coursera or Newspaper.

Proposed bundled binary location:

```text
apps/desktop/src-tauri/binaries/
  yt-dlp-<target-triple>.exe
  deno-<target-triple>.exe
  ffmpeg-<target-triple>.exe
  ffprobe-<target-triple>.exe
```

Tauri `bundle.externalBin` is the selected packaging mechanism. The configuration uses unsuffixed logical names while release acquisition produces target-triple filenames expected by Tauri. Execution uses the workflow-owned Windows managed-process adapter, not JavaScript or a generic shell plugin. No shell permission or general-purpose command capability is exposed to React.

The existing disabled `Generic Video` navigation entry is replaced with a mounted `youtube` route and `YouTubeView`. Provider IPC lives behind `src/lib/youtube/ipc.ts`; playlist interpretation, ordering, transcript selection, output paths and execution remain Rust-owned.

## 7. Supported URL types

V1 supports canonical YouTube URLs for:

- normal watch pages;
- `youtu.be` short URLs;
- Shorts URLs;
- explicit playlist URLs;
- completed livestream VOD URLs when yt-dlp reports them as downloadable video content.

A watch URL containing a playlist reference is ambiguous. The UI MUST ask the user to choose:

```text
This video only
Entire playlist
```

Unsupported hosts or malformed URLs fail before any subprocess is launched.

## 8. User flow

### 8.1 Start

The user chooses **Add -> YouTube** and pastes a URL.

The screen initially shows:

- URL field;
- `Scan` action;
- concise supported-source hint.

### 8.2 Scan

For a single video, LinkVault retrieves video metadata and transcript availability.

For a playlist, LinkVault first performs a flat playlist scan and shows:

- order;
- thumbnail;
- title;
- channel/uploader;
- duration when available;
- availability state;
- selected checkbox.

Caption inspection runs only for selected items, with bounded concurrency, so a very large playlist does not immediately trigger a full metadata request for every entry.

Each displayed row has an opaque occurrence ID. Occurrence identity is distinct from video identity because one playlist may contain the same video more than once.

### 8.3 Configure

The user chooses:

- included videos;
- media mode;
- quality cap;
- preferred transcript language;
- fallback transcript languages;
- whether automatic captions may be used when uploader captions are unavailable;
- output folder.

### 8.4 Download

Downloads execute one selected video at a time in playlist order.

The UI shows:

- current item title;
- current phase;
- item bytes/percentage when available;
- overall `completed / selected` count;
- warnings;
- `Pause after current item`;
- `Cancel`.

The playlist list is keyboard-operable and virtualized when needed. Scan completion, warnings and throttled progress changes use an accessible live region; focus returns to the initiating control after modal choices and cancellation. The mounted view MUST pass narrow, compact and wide container checks without horizontal control loss.

## 9. Scan behavior

The yt-dlp adapter MUST consume machine-readable output, not human console text.

Single-video discovery uses `-J` / `--dump-single-json`. Playlist discovery uses flat-playlist mode with one LinkVault-prefixed JSON record per occurrence rather than one unbounded whole-playlist JSON object. Each record and the aggregate stream are checked against the separate limits below.

Discovery is bounded by these initial V1 safety ceilings:

```text
MAX_SOURCE_URL_BYTES             4096
MAX_SCAN_ENTRIES                  500
MAX_SELECTED_OCCURRENCES          100
CAPTION_INSPECTION_CONCURRENCY       2
MAX_CAPTION_INSPECTION_CONCURRENCY   4
MAX_DISCOVERY_STDOUT_BYTES     32 MiB
MAX_MACHINE_RECORD_BYTES        4 MiB
MAX_RETAINED_STDERR_BYTES      256 KiB
DISCOVERY_IDLE_TIMEOUT             30 s
DISCOVERY_WALL_TIMEOUT               5 min
DOWNLOAD_IDLE_TIMEOUT                 5 min
MERGE_IDLE_TIMEOUT                   10 min
```

Media download and merge have no fixed total wall timeout while progress continues; the idle deadlines and user cancellation still apply.

These are safety limits, not performance claims. A change requires a reviewed spec update and release-build measurement. Oversized, truncated, invalid UTF-8 or trailing-garbage machine output fails with a typed error; it is never partially trusted. stdout and stderr are drained concurrently through bounded readers.

For playlist scan:

```text
validate URL
  -> flat playlist metadata scan
  -> normalize playlist items
  -> freeze immutable scan plan
  -> show selection UI
  -> inspect transcript metadata for selected items
```

The UI should not wait for transcript inspection of hundreds of unselected videos.

The backend returns an opaque `scanPlanId`, per-occurrence IDs and an expiry. Plans are held only in Rust memory, expire after 30 minutes, and are lost on application restart. Transcript inspection and download start reference the plan and occurrence IDs; they do not accept arbitrary video URLs or unbound IDs from React.

`occurrenceId` is a SHA-256 over a version byte, canonical source ID, playlist ID or the single-video sentinel, source ordinal and video ID. `sourceSnapshotDigest` deterministically hashes the versioned canonical source/playlist identity and ordered occurrence identity projection. `metadataDigest` separately hashes the normalized video ID, canonical source URL, title, channel identity, duration and availability. The opaque `scanPlanId` is random and does not expose these digests.

If a playlist exceeds `MAX_SCAN_ENTRIES`, the response is explicitly marked `truncated`; only returned occurrences can be selected and the UI must disclose the limit. Start revalidates source identity and selected occurrence membership. A changed video/playlist identity, selected ordinal or availability invalidates the plan with `SCAN_PLAN_STALE`. Title, channel, duration or thumbnail drift with unchanged identity is accepted with `METADATA_DRIFT`, and current normalized metadata is written to the manifest. Selection never changes silently.

React never receives or loads a provider thumbnail URL directly. A Rust-owned local `youtube-thumbnail` protocol accepts only a current `scanPlanId` plus `occurrenceId`, resolves the plan-held URL, requires HTTPS and an explicit YouTube-thumbnail hostname allowlist, and reapplies that allowlist after every redirect. It sends no cookie, authorization or referrer, rejects URL credentials and loopback/private/link-local destinations after each DNS resolution, applies bounded redirects/bytes/time, verifies an allowed image MIME and decoded dimensions, and returns cached local bytes or a non-image fallback. Provider redirects and metadata cannot select a local-file, arbitrary public host or local-network target. Raw thumbnail URLs never enter React, logs, events, manifests or persisted metadata; metadata records only availability and an optional fetched-content hash. Until the bounded local protocol exists, the mounted UI renders a placeholder rather than a remote image.

## 10. Transcript model

yt-dlp exposes uploader captions and automatic captions separately. LinkVault V1 maps these to two product-facing source types:

```text
Uploader-provided
Automatic
```

V1 MUST NOT claim it can perfectly distinguish every YouTube auto-generated source track from every auto-translated automatic track.

The normalized track model should include at least:

```ts
interface YouTubeTranscriptTrack {
  trackKey: string;
  languageTag: string;
  displayLanguage: string;
  source: "uploader" | "automatic";
  isLikelyTranslated: boolean;
  formats: string[];
}
```

`isLikelyTranslated` is informational and MUST NOT be used as a guarantee.

`trackKey` is an opaque identifier scoped to the immutable scan plan and derived from normalized language/source/format identity, never a temporary signed subtitle URL. React never receives or returns a subtitle URL. Execution refreshes provider metadata and resolves the key to a current helper-owned track; disappearance follows the configured missing-transcript policy.

`live_chat` MUST be excluded from transcript choices.

## 11. Transcript selection

V1 supports one transcript preference chain per download session.

Recommended defaults:

```text
Preferred language:
  application language

Fallback languages:
  video source/original language when known
  English

Source priority:
  uploader-provided
  automatic

Automatic captions:
  allowed

Missing transcript:
  continue with warning
```

The language picker should display coverage for the currently selected playlist items, for example:

```text
English · Uploader-provided   18 / 24
English · Automatic           24 / 24
Japanese · Uploader-provided   8 / 24
```

The provider MUST resolve the final transcript track separately for each video. It MUST NOT assume one exact language tag exists for every playlist item.

V1 MUST NOT use `--sub-langs all` as its default behavior.

Selection is deterministic. User preference tags are normalized for comparison but are otherwise exact; V1 does not invent an implicit primary-language fallback. For each explicitly ordered language, uploader captions win over automatic captions when both are allowed. Ties prefer a non-likely-translated track, then a VTT-capable track, then lexical `trackKey` order. The selected `trackKey`, effective language tag and source are written to the item manifest.

## 12. Transcript artifacts

When a transcript is selected and available, LinkVault stores:

```text
<lesson-slug>.<language-tag>.vtt
<lesson-slug>.transcript.json
```

The VTT file is the raw downloaded subtitle artifact.

The normalized JSON is a LinkVault-owned projection:

```ts
interface TranscriptCue {
  startMs: number;
  endMs: number;
  text: string;
}

interface NormalizedTranscript {
  schemaVersion: 1;
  provider: "youtube";
  videoId: string;
  languageTag: string;
  source: "uploader" | "automatic";
  sourceTrackKey: string;
  sourceVttSha256: string;
  cues: TranscriptCue[];
}
```

V1 normalization does not deduplicate rolling-caption text. It parses WebVTT cue timestamps and settings, omits `NOTE`, `STYLE` and `REGION` blocks from the projection, removes cue formatting tags through a WebVTT-aware parser, decodes supported WebVTT entities, normalizes cue line endings to `\n`, and retains cue line breaks. It never evaluates or renders subtitle markup as HTML.

Malformed timestamps, integer overflow, invalid UTF-8 or a raw VTT larger than 16 MiB fail normalization with a typed item error. The raw VTT remains byte-unchanged and its SHA-256 is recorded. Normalization output is written to staging and published only after schema validation.

## 13. Download modes

V1 supports:

```text
Video + transcript
Video only
Transcript only
```

Transcript-only mode MUST skip media transfer when yt-dlp supports the requested transcript scan/download path.

## 14. Quality policy

Expose user-friendly caps rather than raw yt-dlp format IDs:

```text
Best
Up to 2160p
Up to 1440p
Up to 1080p   (default)
Up to 720p
Up to 480p
```

`formatPolicyVersion: 1` builds a deterministic expression from the cap and current metadata:

1. prefer a complete MP4 with H.264 video and AAC audio at or below the cap;
2. otherwise prefer separate H.264 video and AAC audio streams that can be merged into MP4 without re-encoding;
3. otherwise select the best video/audio streams at or below the cap and let yt-dlp choose a compatible merge container; and
4. for `Best`, remove the height cap but retain the same compatibility ordering.

If only VP9, AV1 or another non-preferred codec is available, the provider may save it without re-encoding but records a `PLAYBACK_COMPATIBILITY_WARNING` containing safe codec/container names. The UI MUST NOT promise OS-native playback for that fallback.

The provider MUST allow yt-dlp/FFmpeg to merge separate video/audio streams when required.

V1 should prefer remux/merge behavior and MUST NOT silently perform a long video re-encode merely to satisfy a preferred container.

FFprobe verification requires a readable container, at least one expected stream, finite non-negative duration when the source reports duration, and no staging path escape. When yt-dlp supplies reliable size estimates, LinkVault performs a conservative free-space preflight that includes separate streams, merge staging and metadata overhead; a later disk-full failure remains a typed item or session error.

## 15. Output organization

For a single video:

```text
<output>/
  <video-title> [<video-id>]/
    <video-file>
    <video-title>.<lang>.vtt
    <video-title>.transcript.json
    metadata.json
    manifest.json
```

For a playlist:

```text
<output>/
  <playlist-title> [<playlist-id>]/
    001 - <video-title> [<video-id>]/
    002 - <video-title> [<video-id>]/
    ...
```

The app-owned safe-filesystem service converts the selected folder into a validated `OutputRoot` before launching a helper. It rejects a file, symlink, junction or other Windows reparse point as the root; safely creates missing directories; canonicalizes each created component; opens root/staging directory handles without delete sharing; records their volume/file identity; and rechecks identity, reparse state and containment immediately before helper launch, after helper exit and before publication. The YouTube provider supplies only sanitized relative-path and artifact plans; it does not open an unvalidated user root itself.

Rust-owned filename logic rejects traversal, absolute child paths, device paths, alternate data streams, control characters, Windows reserved names and components ending in a dot or space. V1 accepts only canonical local drive roots; UNC, device and extended-prefix inputs are rejected. Sanitized title components are capped at 80 UTF-16 code units while preserving the stable ID suffix, and staging/final absolute paths are capped at 240 UTF-16 code units. An unsafe path that cannot be shortened within those rules returns `OUTPUT_PATH_INVALID`. Provider titles and language tags are never used directly as unrestricted paths.

Every attempt receives a cryptographically unpredictable, exclusively created, initially empty staging directory on the same volume beneath:

```text
<output>/.linkvault-staging/youtube/<occurrence-id>/<artifact-fingerprint>/<attempt-id>/
```

The helper writes only inside that clean attempt directory. Preserved partials are never reused in place: the safe-filesystem service opens each matching regular file without following reparse points, verifies its file/volume identity and copies it into the clean attempt before launch. It rejects every pre-existing or helper-created descendant that is a symlink, junction or other reparse point; leaf and nested identities are opened/rechecked before hashing and publication. A detected swap terminates the managed job and returns `SAFE_FILESYSTEM_VIOLATION`.

LinkVault verifies media with FFprobe, validates VTT/JSON/metadata, records sizes and SHA-256 values, flushes files where supported, and atomically renames the complete item directory into its final location. A final directory without a matching verified manifest projection causes `OUTPUT_COLLISION`; LinkVault never silently overwrites it. A matching verified manifest returns a `skipped_existing` item result after re-verification.

`planFingerprint` identifies the whole transient run plan for event/audit correlation. `artifactFingerprint` is the SHA-256 of a version byte, canonical `occurrenceId` and `videoId`, effective mode, format policy/height cap, semantic transcript selection or none, and `helperLockDigest`. It excludes `runId`, `scanPlanId`, whole-run selection/order, output root and mutable display metadata. Start clones a run-owned immutable plan before returning; expiry or eviction of the discovery cache cannot change an active run. Manifest compatibility compares exactly `schemaVersion`, provider, `artifactFingerprint`, occurrence/video/playlist identity, mode, format policy, selected semantic transcript track, helper lock digest and every artifact size/hash. Audit-only timestamps or a different run-plan fingerprint never invalidate otherwise identical item output.

The versioned item manifest contains at least:

```ts
interface YouTubeArtifactManifest {
  schemaVersion: 1;
  provider: "youtube";
  sourceSnapshotDigest: string;
  artifactFingerprint: string;
  occurrenceId: string;
  videoId: string;
  playlistId: string | null;
  playlistIndex: number | null;
  mode: "video_and_transcript" | "video_only" | "transcript_only";
  formatPolicyVersion: 1;
  selectedTranscript: {
    trackKey: string;
    languageTag: string;
    source: "uploader" | "automatic";
  } | null;
  helperLockDigest: string;
  artifacts: Array<{
    kind: "media" | "vtt" | "transcript_json" | "metadata";
    relativePath: string;
    sizeBytes: number;
    sha256: string;
  }>;
  status: "verified";
}
```

## 16. Process invocation

Rust owns all subprocess construction. The provider returns a typed helper kind plus `Vec<OsString>` arguments; it cannot supply an executable path. For every scan, inspection, download, direct FFmpeg and FFprobe launch, the workflow managed-process adapter resolves an absolute bundled path and never searches `PATH`. It opens the executable and executable-loaded companion assets with sharing that denies replacement, validates final path, file/volume identity, exact size, SHA-256 and PE target architecture against the embedded lock, and holds those identities through process creation and use. Before starting yt-dlp specifically, the adapter performs that verification for yt-dlp **and every delegated Deno/FFmpeg/FFprobe executable and loaded asset named in its controlled argv**, then holds all of those non-replaceable identities until the entire yt-dlp Job exits and readers join. Direct later launches are independently verified/held again. Any mismatch or replacement attempt fails closed before network or artifact work.

On Windows the same fail-closed sequence applies to yt-dlp, direct FFmpeg, FFprobe and every future helper: create a kill-on-close Job Object with breakaway disabled, create the process suspended, assign/verify it in the Job Object, start concurrent bounded stdout/stderr readers, then resume its primary thread. Descendants such as FFmpeg inherit containment. Failure to create/configure/assign/verify the Job returns `PROCESS_CONTAINMENT_FAILED`; reader or resume failure returns `HELPER_START_FAILED`. Either path terminates the suspended child when one exists, joins started readers and closes all handles. Windows argument serialization follows one audited argv quoting implementation and never invokes `cmd.exe`, PowerShell or another shell. Direct `Command::new`, `tokio::process::Command`, `CreateProcessW`, shell or `PATH` fallback outside this adapter is prohibited.

Every yt-dlp invocation MUST:

- pass arguments as an argument vector, never through a shell command string;
- use LinkVault-controlled output paths;
- ignore user/global yt-dlp configuration;
- disable plugin discovery;
- disable self-update;
- point yt-dlp at the bundled Deno runtime;
- point yt-dlp at the bundled FFmpeg directory;
- use an app-owned cache and temporary directory rather than yt-dlp user-global locations;
- use bounded stdout/stderr processing;
- emit machine-readable progress when progress is required.

The effective safety options should include the equivalent of:

```text
--ignore-config
--no-plugin-dirs
--no-update
--js-runtimes deno:<bundled-deno-path>
--ffmpeg-location <bundled-ffmpeg-directory>
--cache-dir <app-owned-helper-cache>
```

Remote/external component loading must remain disabled unless a later reviewed change explicitly permits it.

No V1 UI field may append arbitrary yt-dlp arguments.

The child environment is an allowlist of required Windows variables plus app-owned `TEMP`/`TMP`. It excludes user-provided executable, plugin, Python and yt-dlp configuration paths. Full command arguments, signed media URLs and unredacted stderr are never logged.

## 17. Progress

The adapter uses yt-dlp `--progress-template` and explicit `--print` records with LinkVault-owned prefixes. It never scrapes the normal progress bar. Machine records are decoded into enums and bounded numeric fields before entering runtime state.

Run states are:

```text
running
pause_requested
paused
cancelling
completed
completed_with_warnings
failed
cancelled
```

Item states are `pending`, `running`, `completed`, `completed_with_warnings`, `failed`, `cancelled`, `skipped` and `skipped_existing`. Item phases are `waiting`, `transcript`, `media`, `merging`, `normalizing_transcript`, `verifying`, `completed`, `warning`, `failed` and `cancelled`.

Every accepted mutation increments a strictly increasing `revision`. The runtime emits `linkvault://youtube-run-changed` after committing the new in-memory snapshot. Non-terminal byte progress is throttled to at most four events per second; state changes, warnings and terminal events emit immediately. React subscribes before requesting state, ignores a different run or a revision not greater than its last applied revision, and then reconciles the greater snapshot/event. On mount, route remount, invoke-response loss or a revision gap it calls `get_youtube_download_state` with `runId: null` to discover the active or most-recent run; it never owns a polling or processing loop.

```ts
type YouTubeRunState =
  | "running"
  | "pause_requested"
  | "paused"
  | "cancelling"
  | "completed"
  | "completed_with_warnings"
  | "failed"
  | "cancelled";

type YouTubeItemState =
  | "pending"
  | "running"
  | "completed"
  | "completed_with_warnings"
  | "failed"
  | "cancelled"
  | "skipped"
  | "skipped_existing";

type YouTubeItemPhase =
  | "waiting"
  | "transcript"
  | "media"
  | "merging"
  | "normalizing_transcript"
  | "verifying"
  | "completed"
  | "warning"
  | "failed"
  | "cancelled";

type YouTubeWarningCode =
  | "PLAYLIST_TRUNCATED"
  | "TRANSCRIPT_FALLBACK_USED"
  | "TRANSCRIPT_MISSING"
  | "ITEM_UNAVAILABLE"
  | "METADATA_DRIFT"
  | "PLAYBACK_COMPATIBILITY_WARNING"
  | "PARTIAL_QUARANTINED"
  | "ITEM_FAILED_CONTINUING"
  | "EXISTING_VERIFIED_REUSED";

interface YouTubeProgressEvent {
  schemaVersion: 1;
  runId: string;
  revision: number;
  state: YouTubeRunState;
  item: {
    occurrenceId: string;
    artifactFingerprint: string;
    videoId: string;
    ordinal: number;
    title: string;
    state: YouTubeItemState;
    phase: YouTubeItemPhase;
  } | null;
  progress: {
    bytesCompleted: number | null;
    bytesTotal: number | null;
    fraction: number | null;
  };
  counts: {
    completed: number;
    completedWithWarnings: number;
    selected: number;
    failed: number;
    skipped: number;
    cancelled: number;
  };
  warnings: Array<{
    occurrenceId: string | null;
    code: YouTubeWarningCode;
    message: string;
  }>;
  error: YouTubeError | null;
}

interface YouTubeItemOutcomeSnapshot {
  occurrenceId: string;
  artifactFingerprint: string;
  videoId: string;
  ordinal: number;
  title: string;
  state: YouTubeItemState;
  phase: YouTubeItemPhase;
  warnings: YouTubeWarningCode[];
  error: YouTubeError | null;
  publishedArtifactKinds: Array<"media" | "vtt" | "transcript_json" | "metadata">;
}

interface YouTubeRunSnapshot extends YouTubeProgressEvent {
  clientSubmissionId: string;
  planFingerprint: string;
  items: YouTubeItemOutcomeSnapshot[];
}
```

The reconstructable run snapshot retains one bounded outcome for every selected occurrence (maximum 100), including all typed item warnings/errors and published artifact kinds; it contains no raw stderr, signed URL or executable argument. The event warning array and snapshot item arrays are bounded by the selected-occurrence limit and contain safe messages only. `completed` excludes `completedWithWarnings`. When cancellation commits, every not-yet-started selected occurrence becomes `cancelled` for count reconciliation, so terminal counts always sum to `selected`.

The frontend renders typed progress; it MUST NOT interpret arbitrary stderr strings as application state.

## 18. Pause, cancellation and partial resume

### Pause

V1 pause semantics are **Pause after current item**. `pause_youtube_download` changes `running -> pause_requested`. The active occurrence finishes, then the runtime commits `paused` before admitting another occurrence. `resume_youtube_download` changes `paused -> running`; it may also withdraw `pause_requested -> running` before the safe boundary. LinkVault never suspends an active FFmpeg or yt-dlp process mid-file.

### Cancel

`cancel_youtube_download` is idempotent and changes any non-terminal run to `cancelling`. It closes/terminates the active Job Object, drains readers, awaits descendants and staging cleanup, then commits `cancelled`. A cancellation accepted before terminal commit wins over later helper success. Terminal states are immutable, and no next occurrence may start after cancellation is accepted.

`app::cooperative_exit::CooperativeExit` remains the sole exit authority and becomes a tokenized multi-participant barrier. Its native phase is `Open -> Quiescing -> Draining -> Closed | Blocked`; required participants register in the composition root before `Quiescing`, late registration is rejected, and each unique participant token resolves exactly once. Concurrent exit callers share one attempt/result. The existing renderer participant alone may resolve note durability; the YouTube runtime registers a distinct native participant and never invokes `resolve_cooperative_exit` or `authorize_exit`. A main-window `Close` runs renderer durability and hides to tray without stopping YouTube. Tray Quit, ordinary Exit and updater restart use the same token: renderer durability must succeed first, then native participants run in deterministic order. The YouTube participant atomically enters `ShuttingDown`, stops admission, cancels exact discovery/run work, terminates each Job Object, drains readers, releases caption permits/publication handles and joins cleanup. Only after every required participant succeeds may CooperativeExit issue its single-use exit authorization.

A stale, failed or timed-out participant cannot be overwritten by another result. If renderer durability blocks before native draining, the transient runtime remains in its prior admission state. If graceful YouTube cleanup exceeds its bounded participant deadline, LinkVault force-closes the Job Object and still verifies owned child/reader/handle cleanup; inability to establish cleanup commits `Quarantined`, returns `APP_SHUTDOWN_TIMEOUT`, fails the exit closed and restores/focuses the main window with a restart-required/YouTube-disabled explanation. `Blocked` finalizes and clears only that barrier attempt token. A later Quit/updater request receives a new token and may rerun the quarantined participant's idempotent cleanup verification; success permits exit, while another failure remains blocked. No work command can move `Quarantined` back to `Idle`. Updater installation/restart must request this same Exit path and cannot bypass the barrier. Tests combine a dirty clipping note with an active child/grandchild and exercise Close, simultaneous Quit, updater, late registration, timeouts/failure ordering and a second exit attempt.

### Partial resume

LinkVault preserves partial files only inside service-controlled staging. Rerunning an item imports partial data into a new clean attempt only when the exact item-artifact match projection succeeds: schema/provider, video and occurrence identity, effective mode, format policy/height cap, semantic transcript track and helper-lock digest. Run ID, scan-plan ID, whole-run selection and output root never participate. Identity-mismatched or reparse-containing staging is quarantined from automatic reuse and reported as a typed warning.

V1 does not promise automatic continuation after an application crash or OS restart.

Allowed run transitions are checked by one Rust transition table:

```text
running -> pause_requested | cancelling | completed | completed_with_warnings | failed
pause_requested -> running | paused | cancelling | completed | completed_with_warnings | failed
paused -> running | cancelling
cancelling -> cancelled
```

No transition leaves a terminal state. A second start while a run is non-terminal returns `RUN_ALREADY_ACTIVE` without launching a helper.

Item transitions are `pending -> running | skipped | skipped_existing | cancelled` and `running -> completed | completed_with_warnings | failed | cancelled`. Item terminal states are immutable. An item-scoped failure commits before the next pending occurrence is admitted.

## 19. Error model

The provider returns stable typed errors rather than raw yt-dlp messages.

```ts
interface YouTubeError {
  code: YouTubeErrorCode;
  scope: "scan" | "run" | "item";
  retryable: boolean;
  safeMessage: string;
  occurrenceId: string | null;
}
```

Initial error classes:

```text
INVALID_URL
UNSUPPORTED_URL
SCAN_LIMIT_EXCEEDED
SCAN_PLAN_STALE
SCAN_SELECTION_INVALID
DISCOVERY_NOT_FOUND
OPERATION_ID_REUSED
OPERATION_ID_CAPACITY_EXHAUSTED
RUN_ALREADY_ACTIVE
SUBMISSION_CONFLICT
SUBMISSION_ID_RETIRED
SUBMISSION_ID_CAPACITY_EXHAUSTED
RUNTIME_BUSY
RUNTIME_SHUTTING_DOWN
RUN_NOT_FOUND
STALE_RUN_REVISION
INVALID_RUN_TRANSITION
HELPER_MISSING
HELPER_INTEGRITY_FAILED
HELPER_START_FAILED
PROCESS_CONTAINMENT_FAILED
HELPER_OUTPUT_INVALID
HELPER_TIMED_OUT
SCAN_FAILED
VIDEO_UNAVAILABLE
PLAYLIST_UNAVAILABLE
NO_SELECTED_ITEMS
TRANSCRIPT_UNAVAILABLE
MEDIA_DOWNLOAD_FAILED
TRANSCRIPT_DOWNLOAD_FAILED
MERGE_FAILED
OUTPUT_PATH_INVALID
OUTPUT_REPARSE_POINT
SAFE_FILESYSTEM_VIOLATION
OUTPUT_COLLISION
DISK_FULL
DISK_WRITE_FAILED
TRANSCRIPT_NORMALIZATION_FAILED
OUTPUT_VERIFICATION_FAILED
CANCELLED
APP_SHUTDOWN_TIMEOUT
UNKNOWN_YTDLP_FAILURE
```

A raw stderr excerpt MAY be retained only in bounded redacted diagnostics and MUST NOT be the frontend contract.

One failed or unavailable playlist item SHOULD NOT fail the entire playlist by default. The final session may complete with warnings.

Invalid URL/plan/selection, helper absence or integrity failure, invalid output root, run-state conflict and cancellation are scan/run-scoped and stop admission. Video/transcript/media/normalization/verification failures are item-scoped unless they demonstrate a shared helper or output-root failure. `DISK_FULL`, lost output-root containment and repeated helper-start failure are run-fatal. The mapping is fixture-tested against exit codes and bounded machine/diagnostic records; frontend behavior never depends on English stderr text.

A run is `completed` only when every selected occurrence is `completed`, `skipped` or `skipped_existing` and no warning exists. It is `completed_with_warnings` when all admission has ended without a run-fatal error but at least one warning or item failure exists. It is `failed` only for a run-fatal error. These terminal rules are evaluated once after the final admitted item commits.

## 20. Metadata

V1 should save enough metadata to support future LinkVault library/reader work:

```ts
interface YouTubeVideoMetadata {
  schemaVersion: 1;
  provider: "youtube";
  videoId: string;
  sourceUrl: string;
  title: string;
  channelName: string | null;
  channelId: string | null;
  playlistId: string | null;
  playlistTitle: string | null;
  playlistIndex: number | null;
  durationSeconds: number | null;
  thumbnail: {
    available: boolean;
    fetchedSha256: string | null;
  };
  description: string | null;
  uploadDate: string | null;
  acquiredAt: string;
  occurrenceId: string;
  sourceSnapshotDigest: string;
  artifactFingerprint: string;
  helperLockDigest: string;
  media: {
    container: string;
    videoCodec: string | null;
    audioCodec: string | null;
    width: number | null;
    height: number | null;
    durationMs: number | null;
  } | null;
}
```

`metadata.json` is provider-domain data, not a generic workflow record.

## 21. Tauri command contract

Frozen V1 command surface:

```text
scan_youtube_source
inspect_youtube_transcripts
cancel_youtube_discovery
start_youtube_download
get_youtube_download_state
pause_youtube_download
resume_youtube_download
cancel_youtube_download
```

Conceptual TypeScript contracts:

```ts
interface ScanYouTubeSourceRequest {
  clientOperationId: string;
  url: string;
  playlistMode?: "video" | "playlist";
}

interface ScanYouTubeSourceResponse {
  scanPlanId: string;
  expiresAt: string;
  kind: "video" | "playlist";
  title: string;
  sourceId: string;
  canonicalUrl: string;
  playlistId: string | null;
  truncated: boolean;
  items: YouTubeScanItem[];
}

interface YouTubeScanItem {
  occurrenceId: string;
  videoId: string;
  sourceUrl: string;
  title: string;
  ordinal: number;
  channelName: string | null;
  channelId: string | null;
  durationSeconds: number | null;
  thumbnailAvailable: boolean;
  availability: "available" | "unavailable" | "unknown";
  metadataDigest: string;
}

interface InspectYouTubeTranscriptsRequest {
  clientOperationId: string;
  scanPlanId: string;
  occurrenceIds: string[];
}

interface InspectYouTubeTranscriptsResponse {
  occurrences: Array<{
    occurrenceId: string;
    videoId: string;
    tracks: YouTubeTranscriptTrack[];
  }>;
}

interface StartYouTubeDownloadRequest {
  clientSubmissionId: string;
  scanPlanId: string;
  selectedOccurrenceIds: string[];
  outputDir: string;
  mode: "video_and_transcript" | "video_only" | "transcript_only";
  maxHeight: null | 2160 | 1440 | 1080 | 720 | 480;
  preferredLanguage: string | null;
  fallbackLanguages: string[];
  allowAutomaticCaptions: boolean;
  continueWithoutTranscript: boolean;
}

interface StartYouTubeDownloadResponse {
  clientSubmissionId: string;
  runId: string;
  revision: number;
  scanPlanId: string;
  planFingerprint: string;
  state: "running";
}

interface GetYouTubeDownloadStateRequest {
  runId: string | null;
}

type GetYouTubeDownloadStateResponse = YouTubeRunSnapshot | null;

interface CancelYouTubeDiscoveryRequest {
  clientOperationId: string;
}

interface MutateYouTubeRunRequest {
  runId: string;
  expectedRevision: number;
}

interface CancelYouTubeRunRequest {
  runId: string;
}
```

`scanPlanId`, `occurrenceId`, `runId`, `planFingerprint` and `artifactFingerprint` are opaque Rust-created identifiers. Frontend-created `clientOperationId` and `clientSubmissionId` values are validated UUIDv4 idempotency/cancellation keys, never filesystem or process identifiers. `occurrenceId` includes canonical source identity, playlist identity when present, source ordinal and video ID so duplicate playlist occurrences remain distinct. Unknown, duplicate, expired or cross-plan occurrence IDs are rejected. The backend restores source order; frontend array order never controls execution order.

One mutex-protected state is exactly `Idle`, `Discovering(clientOperationId)`, `Running(runId)`, `ShuttingDown` or `Quarantined`. Scan and inspection atomically change `Idle -> Discovering` before integrity/network work and hold a matching RAII guard; start atomically validates and clones the selected scan data into a run-owned plan while changing `Idle -> Running`. A pre-spawn failure either releases the matching discovery reservation or commits the created run to a typed terminal failure; it cannot leave phantom active work. All competing scan-vs-scan, scan-vs-inspect, inspect-vs-inspect and discovery-vs-start interleavings return `RUNTIME_BUSY` without launching a helper. The single runtime-global caption semaphore permits at most two caption helpers even if an implementation bug attempts multiple calls. Each permit is held through process join/cleanup and released on success, error, cancel, timeout, panic and shutdown; additional selected items remain unspawned in the bounded plan rather than an unbounded permit wait.

`clientOperationId` is single-use for the process lifetime. Before admission, the runtime inserts it into a tombstone set capped at 4,096 entries; reuse returns `OPERATION_ID_REUSED` and cannot become a later operation. Once the cap is reached, `OPERATION_ID_CAPACITY_EXHAUSTED` refuses new discovery until process restart rather than evicting an ID and enabling stale-cancel aliasing. `cancel_youtube_discovery` affects only the exact active operation ID, is idempotent for that ID while cancellation is in flight, and joins its managed helpers before the guard returns to `Idle`; a delayed cancel for a tombstone that is no longer active returns `DISCOVERY_NOT_FOUND` and cannot affect newer work. `ShuttingDown` and `Quarantined` reject every work command. React disables/debounces discovery actions while a command is active. No caller can bypass workflow-owned admission by invoking a provider helper directly.

`planFingerprint` is the SHA-256 of the canonical scan identity, ordered selected occurrences, normalized options, format-policy version and helper-lock digest. It identifies the run plan without defining item reuse. Each stable `artifactFingerprint` uses only the item-level projection defined in Section 15.

Start uses a process-lifetime, no-eviction submission ledger capped at 1,024 client IDs. Each entry permanently retains the canonical request fingerprint and run ID; the full Start receipt/snapshot is retained for the active and most-recent run. Replaying an identical `clientSubmissionId` while its receipt is retained returns the same run ID/receipt and never starts work; different canonical input returns `SUBMISSION_CONFLICT`. After a newer run makes an older receipt unavailable, replay of that older tombstone returns `SUBMISSION_ID_RETIRED`, never a new run. Reaching the cap returns `SUBMISSION_ID_CAPACITY_EXHAUSTED` until process restart rather than evicting an ID. A new submission ID while a run is active returns `RUN_ALREADY_ACTIVE`. `get_youtube_download_state({ runId: null })` discovers the active run or, when idle, the most-recent run; an explicit ID returns that retained run or `RUN_NOT_FOUND`. This makes the run pausable/cancellable after route reload, response loss or terminal-before-replay while keeping retention bounded.

Pause and resume require the expected revision and return the committed `YouTubeRunSnapshot`. Cancel is idempotent and targets the exact run ID; it never affects a successor run. Snapshot/event reconciliation is revision based, so dropped or reordered events cannot lose item outcomes.

These contracts are frozen for V1. A semantic change requires a reviewed PRD update, matching Rust/TypeScript contract tests, and preservation of typed Rust ownership. No command accepts an executable path, argument vector, subtitle URL or generic shell input.

## 22. Functional requirements

- **FR-1:** The app MUST accept supported YouTube video, Shorts and explicit playlist URLs.
- **FR-2:** The backend MUST validate YouTube host and URL shape before launching a helper process.
- **FR-3:** A watch URL that includes playlist context MUST allow the user to choose video-only or full-playlist behavior.
- **FR-4:** Playlist scan MUST preserve source order.
- **FR-5:** Playlist scan SHOULD use flat metadata before full per-item inspection.
- **FR-6:** The user MUST be able to include or exclude individual playlist videos.
- **FR-7:** Transcript inspection MUST distinguish uploader-provided tracks from automatic tracks.
- **FR-8:** Transcript choices MUST exclude `live_chat`.
- **FR-9:** The UI MUST allow a preferred transcript language and zero or more fallback languages.
- **FR-10:** The UI MUST allow automatic captions to be enabled or disabled.
- **FR-11:** Final transcript selection MUST be resolved independently for each video.
- **FR-12:** Missing transcripts MUST be able to degrade to an item warning instead of failing the whole session.
- **FR-13:** V1 MUST support Video + transcript, Video only and Transcript only modes.
- **FR-14:** V1 MUST expose quality caps instead of raw format IDs.
- **FR-15:** The default video quality cap MUST be 1080p.
- **FR-16:** Media downloading MUST support separate audio/video stream merging through bundled FFmpeg.
- **FR-17:** The backend MUST use machine-readable yt-dlp discovery/progress output.
- **FR-18:** Selected occurrences MUST execute sequentially in backend-restored playlist order in V1.
- **FR-19:** The frontend MUST show the current item, current phase and overall completed/selected count.
- **FR-20:** The user MUST be able to request pause after the current item and resume or withdraw that request through explicit commands.
- **FR-21:** Cancellation and the app-owned multi-participant true-exit barrier MUST terminate every direct helper and descendant through a managed Windows Job Object without bypassing clipping-note durability.
- **FR-22:** Partial files MUST be isolated in app-service-controlled staging and imported into a clean attempt only when the stable per-item artifact projection matches.
- **FR-23:** A downloaded transcript MUST preserve raw VTT and generate versioned normalized cue JSON.
- **FR-24:** The app safe-filesystem service MUST reject root and descendant reparse/traversal attacks, keep every helper-visible artifact inside the validated output root and publish verified item directories atomically.
- **FR-25:** yt-dlp invocation MUST ignore user configuration, disable plugins and disable self-update.
- **FR-26:** The React frontend MUST NOT receive arbitrary subprocess or shell execution capability.
- **FR-27:** V1 MUST work on a clean supported Windows installation using only LinkVault-bundled helpers whose executable/assets are lock-verified and identity-held before every launch, including scan.
- **FR-28:** Helper updates MUST ship through normal LinkVault releases rather than yt-dlp self-update.
- **FR-29:** One failed playlist item SHOULD allow remaining selected items to continue unless the failure is session-wide.
- **FR-30:** YouTube MUST NOT import another provider, workflow MUST NOT import/re-export YouTube, and aliases/re-exports MUST NOT bypass either dependency boundary.
- **FR-31:** Scan MUST return an expiring immutable plan with distinct per-occurrence IDs.
- **FR-32:** Inspect and start MUST reject unknown, duplicate, expired or cross-plan occurrence IDs.
- **FR-33:** Start MUST be idempotent by client submission ID, return an opaque run ID/plan fingerprint/initial revision, and allow active or most-recent run discovery after response loss.
- **FR-34:** Pause/resume mutations MUST use run identity and expected revision; cancel MUST target an exact run ID.
- **FR-35:** Run revisions MUST increase monotonically and terminal state MUST be immutable.
- **FR-36:** One atomic admission state MUST serialize scan, inspection, start and shutdown; a competing operation fails without launching a helper and discovery cancellation targets an exact operation ID.
- **FR-37:** Helper acquisition and release packaging MUST be driven by a pinned checksum/license/source lock.
- **FR-38:** Every completed item MUST publish a verified artifact manifest with checksums and helper-lock identity.
- **FR-39:** The mounted YouTube view MUST provide keyboard-operable selection/actions, focus management and bounded live progress announcements.
- **FR-40:** Internal Y0-Y3 implementation/testing MUST have an affirmative owner-risk acceptance with an explicit scope; public packaging, distribution and release MUST remain blocked until the separate Y-PUBLIC-REVIEW artifact and approved user-facing copy exist.

## 23. Non-functional requirements

- **NFR-1:** No shell invocation may be used for yt-dlp, Deno or FFmpeg; the Windows adapter accepts typed argv and uses one audited CreateProcess argument serializer.
- **NFR-2:** No arbitrary yt-dlp option field may be exposed in the V1 UI or IPC request.
- **NFR-3:** stdout/stderr buffers MUST be bounded so a noisy helper cannot grow application memory without limit.
- **NFR-4:** Helper process output MUST NOT block the Tauri UI thread.
- **NFR-5:** Cancel, Quit and updater restart MUST leave no known orphaned yt-dlp/FFmpeg descendant in the normal Windows path.
- **NFR-6:** A 100-item playlist flat scan MUST NOT require full caption inspection of all 100 entries before the selection UI becomes usable.
- **NFR-7:** Transcript normalization MUST be deterministic for the same VTT input and schema version.
- **NFR-8:** Existing LinkedIn, Coursera and Newspaper provider behavior MUST remain unchanged.
- **NFR-9:** The implementation MUST add no plaintext browser-cookie or credential storage.
- **NFR-10:** Third-party binaries and licenses MUST be documented in repository notices before release packaging.
- **NFR-11:** Discovery buffers, record sizes, playlist cardinality and timeouts MUST enforce the published V1 safety ceilings.
- **NFR-12:** Progress events MUST be typed, revisioned, bounded and independent of raw stderr text.
- **NFR-13:** React MUST NOT poll or own work admission; it listens for runtime revisions and requests a reconstructable active/most-recent snapshot on mount, response loss or a revision gap.
- **NFR-14:** Architecture verification MUST discover every provider directory and exercise committed negative fixtures for reverse, relative, braced, aliased and re-export dependency bypasses.
- **NFR-15:** No native/process/path/playback claim may be inferred from a frontend build, fake process adapter or unpackaged helper smoke.

## 24. Acceptance criteria

### AC-1: Single-video scan

Given a supported public YouTube video URL

When the user scans it

Then LinkVault returns normalized video metadata

And available uploader/automatic transcript tracks are displayed

And no media file is downloaded during scan.

### AC-2: Playlist scan and selection

Given a public playlist with at least ten entries

When the user scans it

Then LinkVault shows the entries in playlist order

And the user can deselect individual entries

And LinkVault does not require full transcript inspection of unselected entries before showing the list.

### AC-3: Ambiguous watch URL

Given a watch URL containing playlist context

When LinkVault detects both a video and playlist target

Then the UI asks whether to use only the current video or the entire playlist

And the selected interpretation is used consistently for later scan/download actions.

### AC-4: Transcript source preference

Given one video with uploader-provided English captions and automatic English captions

When uploader-provided captions are preferred

Then LinkVault selects the uploader-provided English track

And stores the source as `uploader` in normalized transcript metadata.

### AC-5: Transcript fallback

Given a selected playlist where one item lacks the preferred language but has a configured fallback

When the session runs

Then that video's fallback track is downloaded

And other videos continue normally.

### AC-6: Missing transcript warning

Given a selected video with no matching transcript

And `continueWithoutTranscript` is true

When the download runs

Then media succeeds if available

And the item records a transcript warning

And the session does not fail solely because of the missing transcript.

### AC-7: 1080p media download

Given a video whose best <=1080p representation requires separate video/audio streams

When the default quality is used

Then yt-dlp downloads compatible streams

And bundled FFmpeg merges them

And LinkVault publishes one playable final media artifact without a silent full re-encode.

### AC-8: Transcript artifacts

Given an available selected transcript

When the item completes

Then the raw VTT exists

And normalized transcript JSON exists

And every normalized cue contains integer start/end milliseconds and text

And the raw VTT has not been rewritten by the normalizer.

### AC-9: Cancellation

Given an active media download or merge

When the user cancels

Then the active helper process tree terminates

And no next playlist item begins

And the UI reaches `cancelled` without an orphaned normal yt-dlp/FFmpeg process.

### AC-10: Clean Windows installation

Given a supported Windows machine without system-installed yt-dlp, Python, Deno or FFmpeg

When LinkVault is installed and a supported public video is scanned and downloaded

Then the operation succeeds using only bundled helper binaries.

### AC-11: Isolation from user yt-dlp configuration

Given a machine with a user yt-dlp config or plugin that changes output behavior

When LinkVault invokes its bundled yt-dlp

Then the LinkVault operation ignores that config/plugin

And output remains controlled by LinkVault's typed request.

### AC-12: Provider isolation

Given the completed YouTube V1 source tree

When architecture verification scans provider imports

Then YouTube does not import LinkedIn, Coursera or Newspaper internals

And workflow does not import or re-export YouTube through a relative, braced, aliased or re-exported path

And no new provider-local durable scheduler or generic job table has been introduced.

And the architecture verifier discovers and scans the YouTube provider instead of passing through a fixed legacy-provider allowlist.

### AC-13: Pause, revision and resume

Given a run with at least two selected occurrences

When the user requests pause with the current run ID and revision

Then the current occurrence may finish

And no next occurrence starts before the runtime commits `paused`

And resume commits a greater revision before admitting the next occurrence

And a lost Start response or route remount can discover the active run and reconstruct all selected-item outcomes before pause/cancel.

### AC-14: Immutable scan selection

Given a playlist containing the same video at two ordinals

When the user selects only one occurrence

Then start downloads only that occurrence in source order

And unknown, duplicate, expired or cross-plan occurrence IDs launch no helper.

### AC-15: Atomic and confined output

Given title collisions, reserved Windows names, Unicode, preserved partials and a hostile reparse-point setup

When an item downloads and verifies

Then every write remains beneath the validated output root

And only a complete verified item directory becomes final

And leaf/nested junction swaps in helper-visible staging terminate the managed job

And an existing unmatched final directory is never overwritten

And a subset rerun after rescan reuses only a matching stable item fingerprint.

### AC-16: True application shutdown

Given a dirty clipping note and yt-dlp with a descendant merger process

When the user quits LinkVault or an updater restart is accepted

Then new item admission stops

And the Job Object terminates the child and grandchild processes

And the renderer durability participant succeeds before the native YouTube participant

And the app-owned barrier authorizes exit exactly once only after bounded cleanup completes

And Close-to-tray leaves the run active while Quit and updater restart use the barrier.

### AC-17: Pinned installed helpers

Given an exact installed Windows candidate with no helper on `PATH`

When scan, inspection, download and direct verification helpers are exercised

Then all four binaries match the committed helper lock and installer inventory

And tampered, missing, replaced or wrong-architecture helpers fail before network discovery or artifact access

And Job creation/configuration/assignment/reader/resume failures leave no suspended child.

### AC-18: Mounted accessible UI

Given the mounted Tauri YouTube route at narrow, compact and wide container widths

When a keyboard-only user scans, selects, configures, pauses, resumes and cancels

Then every control has a visible label and accessible name

And focus, list virtualization, warnings and bounded live progress remain usable without horizontal control loss.

### AC-19: Bounded hostile helper output

Given oversized, truncated, invalid UTF-8, noisy or stalled helper output

When scan or download processes it

Then configured byte/cardinality/idle/wall limits are enforced

And memory remains bounded

And a typed scoped error is returned without interpreting partial data.

### AC-20: Internal owner-risk authorization and public-release gate

Given a candidate containing the YouTube feature

When release verification runs

Then internal implementation validation parses `docs/legal/youtube-v1-approval.md` as an affirmative, non-expired owner-risk acceptance

And its reviewed specification identity, target, internal-only scope and permitted/prohibited content policy match the candidate

And the product-owner sign-off, scope fields and approved internal user-facing copy are non-empty

And the acceptance explicitly states that no legal/counsel approval or platform permission is being claimed

And the first-use acknowledgement is keyboard and screen-reader accessible

And helper execution remains blocked until Y0 creates and verifies the exact helper lock, regardless of owner acceptance

When public packaging, distribution or release validation runs

Then the owner-risk acceptance alone is insufficient and the separate Y-PUBLIC-REVIEW product/legal decision, exact specification/helper-lock identities, approved copy and packaged/native UAT evidence are required

And absence, mismatch, rejection or expiry of either required decision blocks public packaging and release.

## 25. Test matrix

Automated tests should cover at minimum:

### Unit

- URL normalization and host validation;
- video-vs-playlist interpretation;
- immutable scan-plan expiry, run-owned plan pinning and distinct run/item fingerprinting;
- duplicate-video occurrence identity and source-order restoration;
- format-policy generation for each height cap;
- subtitle-track normalization;
- language fallback selection;
- `live_chat` exclusion;
- path sanitization and traversal rejection;
- VTT cue normalization;
- stable error mapping;
- progress-line parsing;
- the complete run/item transition matrix;
- monotonic revisions, stale mutations and terminal-state immutability;
- helper-lock schema/RFC-8785 digest, asset/source size/hash, extraction-member and compatibility validation; and
- exact artifact-manifest match projection and checksum validation.

### Integration with fixtures/fake process adapter

- flat playlist scan;
- transcript inspection;
- sequential item execution;
- lost Start response, terminal-before-replay, identical/conflicting/retired submission IDs, no-eviction ledger capacity, route remount and active/latest snapshot discovery;
- dropped/reordered revision events with reconstructable selected-item outcomes;
- double-start rejection and stale-run cancellation isolation;
- barrier-controlled scan-vs-scan, scan-vs-inspect, inspect-vs-inspect, discovery-vs-start and shutdown interleavings;
- abandoned/panicking discovery guards, single-use/capacity tombstones, delayed exact-operation cancellation and one global caption semaphore;
- one-item failure followed by continuation;
- pause-after-current-item, paused admission and resume;
- cancellation;
- cancellation/completion races and terminal event uniqueness;
- dirty-note plus child/grandchild process termination on Cancel, Quit and updater restart, including participant timeout/failure ordering, quarantine UI and second-exit retry;
- transcript-only mode;
- missing-transcript warning;
- bounded helper output, invalid UTF-8, truncation and timeouts;
- app safe-filesystem root and every-descendant reparse/TOCTOU containment, including leaf/nested swaps during execution;
- title/reserved-name collisions and unmatched final directories;
- compatible/incompatible partial import plus restart/subset reruns with changed run/scan identity;
- FFprobe/normalization/atomic-publication failure injection;
- disk-full cleanup; and
- every-launch missing/tampered/replaced/wrong-architecture helpers, including scan-time replacement and delegated Deno/FFmpeg replacement immediately before child spawn;
- Job creation/configuration/assignment, reader startup and resume failure cleanup; and
- thumbnail localhost/redirect-host escape, oversized/non-image body, timeout and offline fallback.

### Mounted UI/browser-Tauri harness

- the real `youtube` route and IPC adapter are mounted;
- scan/loading/error/empty/truncated-playlist states;
- duplicate occurrence selection and transcript coverage;
- keyboard-only selection, configuration, pause/resume and cancel;
- focus restoration after ambiguity, error and cancellation flows;
- throttled `aria-live` progress and occurrence-associated warnings;
- 100-row virtualization and thumbnail failure fallback;
- narrow, compact and wide container geometry with no hidden controls; and
- first-use legal acknowledgement accessibility.

### Native Windows UAT

- clean installed app with no external dependencies;
- single video;
- 10+ item playlist;
- duplicate playlist occurrence with only one selected;
- separate-stream FFmpeg merge;
- uploaded captions;
- automatic captions;
- missing captions;
- unavailable playlist item;
- cancel during media transfer;
- cancel during merge;
- pause after current item and resume;
- tray close while running followed by true Quit;
- updater restart with an active helper tree;
- dirty clipping note plus active helper Quit/updater ordering;
- lost Start response followed by route remount, state recovery and exact-run cancellation;
- manual rerun after an interrupted `.part` download;
- rescan/subset rerun with a different run plan but matching item artifact fingerprint;
- paths containing spaces and Unicode characters;
- reserved-name, long-path and reparse-point rejection;
- exact installed helper inventory/checksums with no helper on `PATH`;
- H.264/AAC, VP9 and AV1/container verification behavior; and
- no orphaned process after Cancel, Quit or updater restart.

Network-dependent YouTube UAT MUST use authorized/public test content and MUST NOT become the only automated release evidence.

Every implementation slice runs `npm run verify:no-any`, `npm run build`, `npm run verify:architecture`, `npm run verify:persistence`, `npm run verify:ui` and the new focused `npm run verify:youtube` gate. A release candidate additionally runs installer inventory/integrity verification and exact installed native UAT. Fake-process, Vite and manifest evidence remain explicitly separate from packaged/native proof.

## 26. Rollout plan

### Y-EVIDENCE - non-executing legal/supply-chain evidence selection

- select proposed exact helper versions, target asset/source URLs, publisher-reported sizes/hashes, extraction members, compatibility fields, licenses and FFmpeg build configuration;
- commit only the proposed canonical helper lock, notices/source-offer plan and validation schemas/scripts;
- compute the provisional helper-lock digest and pin the exact reviewed specification commit; and
- prepare product scope, UI copy and authorized UAT-content policy for review.

Gate: this phase MUST NOT download, commit, bundle or execute helper binaries, implement provider network/execution code, or ship a YouTube UI. It exists so the approval decision has exact evidence without requiring the action it is meant to authorize.

### Y-OWNER-RISK - internal implementation authorization

- record `docs/legal/youtube-v1-approval.md` as an affirmative product-owner owner-risk acceptance, with date, review/expiry date, reviewed specification identity and approved copy;
- authorize only Y0-Y3 implementation and testing in isolated internal builds; do not authorize public packaging, distribution or release;
- authorize network UAT only with public content the user owns or is authorized to save, and record the first-use acknowledgement and persistent guidance;
- state that no legal/counsel approval, platform permission or third-party-content license is being claimed; and
- confirm that cookie authentication, account use, member/private/paid/age-gated content, DRM/access-control/rate-limit bypass and attempts to evade provider restrictions remain excluded.

Gate: internal validation parses the acceptance rather than checking file presence. Exactly one decision must be selected; the product-owner sign-off, scope/copy fields and no-counsel-claim statement must be non-empty; the decision must be unexpired; and the reviewed specification identity must match. This acceptance authorizes Y0-Y3 internal implementation/testing only. It does not authorize public packaging, distribution, release, account access, restricted-content access or helper execution before the Y0 lock/integrity gate passes. Any material reviewed-spec, helper-lock, component, scope or copy change invalidates the acceptance and requires re-acceptance.

### Y-PUBLIC-REVIEW - separate public distribution authorization

- obtain a separate affirmative product/legal/platform review for public packaging or distribution;
- pin the exact specification commit/blob and helper-lock digest selected by Y-EVIDENCE/Y0;
- approve final public scope, user-facing copy, third-party helper redistribution and authorized UAT policy; and
- retain the V1 prohibitions on cookies/accounts, member/private/paid/age-gated content, DRM/access-control/rate-limit bypass and attempts to evade provider restrictions.

Gate: this review is not satisfied by the owner-risk acceptance, this PRD, the template or an internal build. Public packaging, distribution and release remain blocked until the separate decision is affirmative, current, identity-matched and accompanied by exact packaged/native UAT evidence.

### Y0 - transient contract and helper packaging spike

- implement the ADR-003 transition table, run/revision contracts and fake managed-process adapter;
- implement the atomic admission state, global caption permit, current/latest snapshot recovery and app-owned exit participant barrier;
- make architecture verification discover every provider and exercise reverse/relative/braced/aliased/re-export negative fixtures;
- finalize the helper lock and digest selected by Y-EVIDENCE, the acquisition verifier, licenses, source records and FFmpeg build configuration;
- bundle exact lock-verified yt-dlp, Deno, FFmpeg and FFprobe binaries;
- prove they execute in an installed Windows build;
- verify no system installation is required;
- prove missing/tampered/wrong-architecture binaries fail closed; and
- record installed versions, paths, checksums, sources and licenses.

Gate: contract/fault tests, supply-chain validation, exact installer inventory and clean-Windows helper smoke pass. No YouTube network success is inferred from helper startup alone.

### Y1 - scan and transcript discovery

- URL parsing;
- single-video metadata;
- flat playlist scan;
- item selection;
- immutable expiring scan plans and duplicate occurrence identity;
- transcript inspection;
- language/source UI.

Gate: bounded fake/fixture scan coverage and mounted accessible UI pass before network UAT.

### Y2 - transcript-only execution

- transcript resolution;
- VTT download;
- normalized JSON;
- revisioned progress, pause/resume/cancel and scoped error handling;
- manifest-bound staging and atomic transcript publication; and
- Quit/updater process-tree cleanup.

Transcript-only should be the first end-to-end implementation because it validates most provider boundaries with much smaller artifacts.

Gate: deterministic fixture output, hostile-helper limits, cancellation/shutdown and exact installed transcript-only UAT pass.

### Y3 - media execution

- quality policy;
- sequential media downloads;
- FFmpeg merge;
- output verification;
- pause-after-current-item;
- partial resume behavior.

Gate: format-policy fixtures, FFprobe failure injection, atomic publication, disk-full cleanup and exact installed media UAT pass.

### Y4 - adversarial hardening and public release (separate authorization)

- process-tree cancellation;
- malformed/huge playlist cases;
- noisy stdout/stderr;
- disk-full/write failures;
- path attacks;
- unavailable videos;
- installed-app UAT;
- release notices and licensing review.

Gate: Y4/public release is outside the owner-risk acceptance and cannot start public packaging or distribution without Y-PUBLIC-REVIEW. All automated gates, mounted UI/accessibility checks, separate review artifact verification and exact packaged native UAT must pass. Record the future Phase 2 migration/removal issue for `workflow::transient`; do not dual-write or pre-create durable YouTube tables.

## 27. Future follow-ups

After V1 has real usage, consider separately scoped work for:

- browser profile/cookie authentication;
- account-restricted and member content where legally and technically appropriate;
- auto-translated subtitle UX;
- saving multiple transcript languages per item;
- SRT export;
- embedded subtitles;
- audio-only mode;
- channel feeds;
- live-stream recording;
- Whisper transcript fallback;
- independent helper-component updates;
- migration to the shared durable workflow kernel.

Do not pre-build these systems into V1 unless a concrete implementation blocker proves they are necessary.

## 28. Security and trust boundaries

- Treat the pasted URL, yt-dlp metadata, titles, descriptions and subtitle content as untrusted input.
- Never build shell command strings from provider metadata.
- Never execute user-provided yt-dlp arguments.
- Never load user-installed yt-dlp plugins.
- Never permit downloaded filenames to escape the selected output root.
- Pass opaque safe-filesystem capabilities across app/workflow/provider boundaries, not unvalidated absolute `PathBuf` values. The app service validates every executable, working directory, `TEMP`/`TMP`, cache, Deno/FFmpeg/FFprobe location, staging directory and output path visible through argv or environment.
- Require canonical app-owned containment, stable volume/file identities and no device/ADS/reparse component before spawn, after helper exit and before publication. Unexpected descendants or root/volume replacement terminate the Job and quarantine staging.
- Never load provider thumbnail URLs directly in the WebView; use the bounded Rust-owned protocol.
- Render provider titles, descriptions, warnings and transcript cues as text, never injected HTML.
- Never expose generic sidecar execution directly to React.
- Never log full secret-bearing command arguments if authentication is added in a later phase.
- Keep remote component loading disabled in V1.

## 29. Legal/product messaging

Y-EVIDENCE permits static, non-executing selection of exact helper/source/license facts and produces the canonical proposed lock digest. Gate Y-OWNER-RISK is satisfied by the affirmative `docs/legal/youtube-v1-approval.md` owner-risk acceptance for Y0-Y3 internal implementation/testing only. The acceptance records the exact reviewed PRD identity, decision owner/date, review/expiry date, target/components, permitted/prohibited scope, approved first-use acknowledgement, persistent UI copy and authorized UAT-content policy; it explicitly makes no legal/counsel or platform-permission claim. Candidate validation parses the affirmative owner choice and recomputes the reviewed PRD identity; helper execution separately requires the exact Y0 helper lock and integrity gate. A material PRD, lock, component, scope or copy change triggers re-acceptance. Gate Y-PUBLIC-REVIEW remains a separate requirement for public packaging, distribution and release, including final helper redistribution and legal/product review. The template is `docs/legal/youtube-v1-approval.template.md`; copying it without an affirmative matching decision does not satisfy either gate.

The intended product position is a local archival feature for public content the user owns or is authorized to save. The UI must not claim that LinkVault grants permission to copy third-party content, that user copyright permission necessarily satisfies platform terms, or that the feature bypasses access restrictions.

Cookie authentication, member/private/age-gated access, DRM or paid-access bypass and any attempt to evade provider restrictions remain prohibited in V1. A first-use acknowledgement communicates the approved scope but is not treated as a technical or legal enforcement mechanism.

Any future restricted-content or cookie-authentication proposal requires a new architecture/security review and a new legal/product decision; it cannot enter as a refinement of this V1 contract.

## 30. External implementation references

- yt-dlp repository and option reference: <https://github.com/yt-dlp/yt-dlp>
- yt-dlp external JavaScript runtime / EJS guidance: <https://github.com/yt-dlp/yt-dlp/wiki/EJS>
- Tauri v2 external binaries / sidecars: <https://v2.tauri.app/develop/sidecar/>
- FFmpeg: <https://ffmpeg.org/>
- FFmpeg legal and redistribution guidance: <https://ffmpeg.org/legal.html>
- YouTube Terms of Service: <https://www.youtube.com/static?template=terms>

## 31. Definition of done

The owner-risk acceptance is sufficient to begin Y0-Y3 internal implementation/testing only. YouTube Downloader V1 is complete for public release only when Y-PUBLIC-REVIEW is affirmatively satisfied and an exact clean installed Windows candidate can:

1. scan a public YouTube video or explicit playlist;
2. show ordered playlist occurrences, including duplicates, and let the user select items;
3. inspect uploader-provided and automatic transcript tracks for selected videos;
4. choose preferred and fallback transcript languages;
5. choose video/transcript mode and a simple quality cap;
6. download selected videos sequentially;
7. merge separate media streams through bundled FFmpeg when required;
8. save raw VTT and normalized transcript JSON;
9. report typed revisioned progress and reconstruct every selected-item outcome after response/event loss;
10. pause after the current item and resume without admitting work while paused;
11. cancel, Quit and updater-restart through the app-owned barrier without bypassing note durability or leaving an orphaned helper descendant;
12. use stable item fingerprints, safe-filesystem capabilities and clean staging to atomically publish checksum-verified manifests/artifacts across subset reruns;
13. work without externally installed yt-dlp, Deno, FFmpeg or FFprobe;
14. verify and identity-hold every exact installed helper/loaded asset against the pinned source/checksum/license lock before every launch;
15. ignore user yt-dlp configs/plugins and expose no arbitrary shell surface;
16. atomically serialize/cancel discovery, start, run and shutdown identities and reject stale/cross-plan mutations without launching or killing the wrong process;
17. enforce hostile-output, timeout, thumbnail-network, path, reparse, collision and disk-failure boundaries;
18. provide a mounted keyboard/screen-reader-accessible UI at narrow, compact and wide container widths;
19. pass architecture dependency negative fixtures, persistence, no-any, build, UI, focused YouTube and installer-integrity gates;
20. record exact packaged/native UAT separately from fake, browser and manifest evidence;
21. leave existing providers and persistence behavior unchanged; and
22. track removal of the ADR-003 transient bridge after migration to the durable workflow kernel.
