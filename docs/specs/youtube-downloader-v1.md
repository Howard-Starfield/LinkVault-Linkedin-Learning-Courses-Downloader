# PRD: YouTube Downloader V1

**Author:** LinkVault engineering  
**Date:** 2026-08-20  
**Status:** Proposed  
**Target:** LinkVault desktop / Windows-first  
**Provider owner:** `apps/desktop/src-tauri/src/providers/youtube/`  
**Related architecture:** [ADR-001: Unified workflow modular monolith](../architecture/adr-001-unified-workflow-modular-monolith.md)

## 1. Summary

Add YouTube as a first-class LinkVault provider for downloading public YouTube videos and playlists together with uploader-provided or YouTube automatic transcripts.

V1 is deliberately narrow. It directly integrates bundled `yt-dlp`, Deno, FFmpeg and FFprobe helpers behind Rust-owned commands. It does **not** introduce a generic toolchain manager, independent binary updater, arbitrary yt-dlp command surface, browser-cookie authentication system, or a second durable workflow engine.

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

## 5. Architecture fit and migration rule

LinkVault's accepted architecture says providers should eventually execute behind the shared durable workflow kernel. The current repository still has only the ownership boundary for that kernel.

YouTube V1 therefore uses a constrained provider-local execution session:

- no provider-local scheduler;
- no generic durable job/event tables;
- no background polling loop owned by React;
- no cross-provider imports;
- no cross-restart workflow guarantee.

The YouTube provider owns discovery, validation, yt-dlp invocation, transcript normalization and item execution. When the shared workflow runtime is ready, these operations should be adapted behind the shared planner/executor contracts rather than rewritten.

## 6. Proposed source layout

```text
apps/desktop/src-tauri/src/providers/youtube/
  mod.rs
  commands.rs
  models.rs
  urls.rs
  ytdlp.rs
  captions.rs
  paths.rs
  errors.rs
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

The exact Tauri packaging mechanism may use `bundle.externalBin`; execution remains Rust-owned. The React frontend MUST NOT receive a general-purpose shell/command capability.

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

## 9. Scan behavior

The yt-dlp adapter MUST consume machine-readable output, not human console text.

Preferred discovery mechanisms are yt-dlp JSON output such as `-J` / `--dump-single-json`, with flat-playlist mode where appropriate.

For playlist scan:

```text
validate URL
  -> flat playlist metadata scan
  -> normalize playlist items
  -> show selection UI
  -> inspect transcript metadata for selected items
```

The UI should not wait for transcript inspection of hundreds of unselected videos.

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
  languageTag: string;
  displayLanguage: string;
  source: "uploader" | "automatic";
  isLikelyTranslated: boolean;
  formats: string[];
}
```

`isLikelyTranslated` is informational and MUST NOT be used as a guarantee.

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
  cues: TranscriptCue[];
}
```

Normalization MAY remove duplicate rolling-caption text, but the raw VTT MUST remain unchanged.

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

The provider builds a format-selection expression based on the cap and current video metadata.

The provider MUST allow yt-dlp/FFmpeg to merge separate video/audio streams when required.

V1 should prefer remux/merge behavior and MUST NOT silently perform a long video re-encode merely to satisfy a preferred container.

## 15. Output organization

For a single video:

```text
<output>/
  <video-title>/
    <video-file>
    <video-title>.<lang>.vtt
    <video-title>.transcript.json
    metadata.json
```

For a playlist:

```text
<output>/
  <playlist-title>/
    001 - <video-title>/
    002 - <video-title>/
    ...
```

Filesystem names MUST be sanitized by Rust-owned path logic. Provider titles MUST NOT be used directly as unrestricted paths.

## 16. Process invocation

Rust owns all subprocess construction.

Every yt-dlp invocation MUST:

- pass arguments as an argument vector, never through a shell command string;
- use LinkVault-controlled output paths;
- ignore user/global yt-dlp configuration;
- disable plugin discovery;
- disable self-update;
- point yt-dlp at the bundled Deno runtime;
- point yt-dlp at the bundled FFmpeg directory;
- use bounded stdout/stderr processing;
- emit machine-readable progress when progress is required.

The effective safety options should include the equivalent of:

```text
--ignore-config
--no-plugin-dirs
--no-update
--js-runtimes deno:<bundled-deno-path>
--ffmpeg-location <bundled-ffmpeg-directory>
```

Remote/external component loading must remain disabled unless a later reviewed change explicitly permits it.

No V1 UI field may append arbitrary yt-dlp arguments.

## 17. Progress

The adapter SHOULD use yt-dlp `--progress-template` or explicit `--print` output rather than scraping the normal progress bar.

Normalized phases:

```text
scanning
waiting
transcript
media
merging
normalizing_transcript
verifying
completed
warning
failed
cancelled
```

The frontend renders typed progress events; it MUST NOT interpret arbitrary stderr strings as application state.

## 18. Pause, cancellation and partial resume

### Pause

V1 pause semantics are **Pause after current item**. LinkVault does not need to suspend an active FFmpeg or yt-dlp process mid-file.

### Cancel

Cancel MUST terminate the active yt-dlp process and descendant FFmpeg processes. On Windows the implementation SHOULD use a process-tree containment mechanism such as a Job Object so cancellation does not leave an orphaned merger process.

### Partial resume

LinkVault SHOULD preserve yt-dlp partial files and normal resume behavior. Rerunning the same interrupted item should reuse compatible partial data when yt-dlp can do so.

V1 does not promise automatic continuation after an application crash or OS restart.

## 19. Error model

The provider returns stable typed errors rather than raw yt-dlp messages.

Initial error classes:

```text
INVALID_URL
UNSUPPORTED_URL
HELPER_MISSING
HELPER_START_FAILED
SCAN_FAILED
VIDEO_UNAVAILABLE
PLAYLIST_UNAVAILABLE
NO_SELECTED_ITEMS
TRANSCRIPT_UNAVAILABLE
MEDIA_DOWNLOAD_FAILED
TRANSCRIPT_DOWNLOAD_FAILED
MERGE_FAILED
OUTPUT_PATH_INVALID
DISK_WRITE_FAILED
CANCELLED
UNKNOWN_YTDLP_FAILURE
```

A raw stderr excerpt MAY be retained only in bounded redacted diagnostics and MUST NOT be the frontend contract.

One failed or unavailable playlist item SHOULD NOT fail the entire playlist by default. The final session may complete with warnings.

## 20. Metadata

V1 should save enough metadata to support future LinkVault library/reader work:

```ts
interface YouTubeVideoMetadata {
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
  thumbnailUrl: string | null;
  description: string | null;
  uploadDate: string | null;
}
```

`metadata.json` is provider-domain data, not a generic workflow record.

## 21. Tauri command contract

Initial command surface:

```text
scan_youtube_source
inspect_youtube_transcripts
start_youtube_download
get_youtube_download_state
cancel_youtube_download
```

Conceptual TypeScript contracts:

```ts
interface ScanYouTubeSourceRequest {
  url: string;
  playlistMode?: "video" | "playlist";
}

interface ScanYouTubeSourceResponse {
  kind: "video" | "playlist";
  title: string;
  sourceId: string;
  items: YouTubeScanItem[];
}

interface YouTubeScanItem {
  videoId: string;
  sourceUrl: string;
  title: string;
  index: number;
  durationSeconds: number | null;
  thumbnailUrl: string | null;
  availability: "available" | "unavailable" | "unknown";
}

interface InspectYouTubeTranscriptsRequest {
  videoUrls: string[];
}

interface InspectYouTubeTranscriptsResponse {
  videos: Array<{
    videoId: string;
    tracks: YouTubeTranscriptTrack[];
  }>;
}

interface StartYouTubeDownloadRequest {
  sourceUrl: string;
  playlistMode: "video" | "playlist";
  selectedVideoIds: string[];
  outputDir: string;
  mode: "video_and_transcript" | "video_only" | "transcript_only";
  maxHeight: null | 2160 | 1440 | 1080 | 720 | 480;
  preferredLanguage: string | null;
  fallbackLanguages: string[];
  allowAutomaticCaptions: boolean;
  continueWithoutTranscript: boolean;
}

interface YouTubeDownloadState {
  state: "idle" | "running" | "pause_requested" | "completed" | "completed_with_warnings" | "failed" | "cancelled";
  currentVideoId: string | null;
  currentTitle: string | null;
  phase: string | null;
  itemProgress: number | null;
  completedItems: number;
  selectedItems: number;
  warnings: Array<{ videoId: string | null; code: string; message: string }>;
}
```

These contracts may be refined during implementation, but changes MUST preserve typed Rust ownership and avoid a generic shell API.

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
- **FR-18:** Selected videos MUST execute sequentially in playlist order in V1.
- **FR-19:** The frontend MUST show the current item, current phase and overall completed/selected count.
- **FR-20:** The user MUST be able to request pause after the current item.
- **FR-21:** Cancellation MUST terminate the active yt-dlp process and descendant merger processes.
- **FR-22:** Compatible yt-dlp partial files SHOULD be preserved for manual retry/resume.
- **FR-23:** A downloaded transcript MUST preserve raw VTT and generate versioned normalized cue JSON.
- **FR-24:** Output paths MUST be sanitized and remain inside the user-selected output root.
- **FR-25:** yt-dlp invocation MUST ignore user configuration, disable plugins and disable self-update.
- **FR-26:** The React frontend MUST NOT receive arbitrary subprocess or shell execution capability.
- **FR-27:** V1 MUST work on a clean supported Windows installation using LinkVault-bundled helper binaries.
- **FR-28:** Helper updates MUST ship through normal LinkVault releases rather than yt-dlp self-update.
- **FR-29:** One failed playlist item SHOULD allow remaining selected items to continue unless the failure is session-wide.
- **FR-30:** YouTube provider code MUST NOT import internals from another provider.

## 23. Non-functional requirements

- **NFR-1:** No shell-string construction may be used for yt-dlp, Deno or FFmpeg invocation.
- **NFR-2:** No arbitrary yt-dlp option field may be exposed in the V1 UI or IPC request.
- **NFR-3:** stdout/stderr buffers MUST be bounded so a noisy helper cannot grow application memory without limit.
- **NFR-4:** Helper process output MUST NOT block the Tauri UI thread.
- **NFR-5:** Cancellation MUST leave no known orphaned yt-dlp/FFmpeg process in the normal Windows cancellation path.
- **NFR-6:** A 100-item playlist flat scan MUST NOT require full caption inspection of all 100 entries before the selection UI becomes usable.
- **NFR-7:** Transcript normalization MUST be deterministic for the same VTT input and schema version.
- **NFR-8:** Existing LinkedIn, Coursera and Newspaper provider behavior MUST remain unchanged.
- **NFR-9:** The implementation MUST add no plaintext browser-cookie or credential storage.
- **NFR-10:** Third-party binaries and licenses MUST be documented in repository notices before release packaging.

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

And no new provider-local durable scheduler or generic job table has been introduced.

## 25. Test matrix

Automated tests should cover at minimum:

### Unit

- URL normalization and host validation;
- video-vs-playlist interpretation;
- format-policy generation for each height cap;
- subtitle-track normalization;
- language fallback selection;
- `live_chat` exclusion;
- path sanitization and traversal rejection;
- VTT cue normalization;
- stable error mapping;
- progress-line parsing.

### Integration with fixtures/fake process adapter

- flat playlist scan;
- transcript inspection;
- sequential item execution;
- one-item failure followed by continuation;
- pause-after-current-item;
- cancellation;
- transcript-only mode;
- missing-transcript warning;
- bounded helper output.

### Native Windows UAT

- clean installed app with no external dependencies;
- single video;
- 10+ item playlist;
- separate-stream FFmpeg merge;
- uploaded captions;
- automatic captions;
- missing captions;
- unavailable playlist item;
- cancel during media transfer;
- cancel during merge;
- manual rerun after an interrupted `.part` download;
- paths containing spaces and Unicode characters.

Network-dependent YouTube UAT MUST use authorized/public test content and MUST NOT become the only automated release evidence.

## 26. Rollout plan

### Y0 - helper packaging spike

- bundle exact yt-dlp, Deno, FFmpeg and FFprobe binaries;
- prove they execute in an installed Windows build;
- verify no system installation is required;
- record versions, source locations, checksums and licenses.

### Y1 - scan and transcript discovery

- URL parsing;
- single-video metadata;
- flat playlist scan;
- item selection;
- transcript inspection;
- language/source UI.

### Y2 - transcript-only execution

- transcript resolution;
- VTT download;
- normalized JSON;
- progress/cancel/error handling.

Transcript-only should be the first end-to-end implementation because it validates most provider boundaries with much smaller artifacts.

### Y3 - media execution

- quality policy;
- sequential media downloads;
- FFmpeg merge;
- output verification;
- pause-after-current-item;
- partial resume behavior.

### Y4 - adversarial hardening and release

- process-tree cancellation;
- malformed/huge playlist cases;
- noisy stdout/stderr;
- disk-full/write failures;
- path attacks;
- unavailable videos;
- installed-app UAT;
- release notices and licensing review.

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
- Never expose generic sidecar execution directly to React.
- Never log full secret-bearing command arguments if authentication is added in a later phase.
- Keep remote component loading disabled in V1.

## 29. Legal/product messaging

LinkVault should present YouTube downloading as a local archival feature for content the user owns or is authorized to save. The product must not claim that LinkVault grants permission to copy third-party content or bypasses platform rights restrictions.

This is product guidance, not a DRM-bypass feature requirement.

## 30. External implementation references

- yt-dlp repository and option reference: <https://github.com/yt-dlp/yt-dlp>
- yt-dlp external JavaScript runtime / EJS guidance: <https://github.com/yt-dlp/yt-dlp/wiki/EJS>
- Tauri v2 external binaries / sidecars: <https://v2.tauri.app/develop/sidecar/>
- FFmpeg: <https://ffmpeg.org/>

## 31. Definition of done

YouTube Downloader V1 is complete when a clean installed Windows build can:

1. scan a public YouTube video or explicit playlist;
2. show ordered playlist entries and let the user select items;
3. inspect uploader-provided and automatic transcript tracks for selected videos;
4. choose preferred and fallback transcript languages;
5. choose video/transcript mode and a simple quality cap;
6. download selected videos sequentially;
7. merge separate media streams through bundled FFmpeg when required;
8. save raw VTT and normalized transcript JSON;
9. report typed progress, warnings and failures;
10. pause after the current item;
11. cancel without leaving a normal orphaned helper process tree;
12. work without externally installed yt-dlp, Deno or FFmpeg;
13. ignore user yt-dlp configs/plugins and expose no arbitrary shell surface; and
14. leave existing providers and persistence behavior unchanged.
