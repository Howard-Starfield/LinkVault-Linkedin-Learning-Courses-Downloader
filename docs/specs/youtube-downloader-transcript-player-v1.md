# YouTube Downloads and Synchronized Transcript Player V1

**Status:** Proposed  
**Date:** 2026-08-20  
**Owner:** LinkVault  
**Scope:** YouTube download discovery, local media playback, WebVTT transcript display, and click-to-seek interaction  
**Implementation authorization:** This PRD authorizes design and dependency spikes only until its phase gates are approved.

Related architecture:

- [Architecture source of truth](../architecture/README.md)
- [ADR-001: Unified workflow modular monolith](../architecture/adr-001-unified-workflow-modular-monolith.md)
- [Frontend/Rust ownership boundary](../architecture/frontend-rust-ownership-boundary.md)

---

## 1. Executive decision

LinkVault should add YouTube as a first-class provider with a local player that displays a synchronized transcript beside the video.

The V1 technical direction is:

1. Bundle `yt-dlp`, Deno, FFmpeg, and FFprobe directly with the desktop application.
2. Do **not** create a generic toolchain manager, independent binary updater, or user-configurable executable-path system.
3. Download WebVTT (`.vtt`) captions directly with yt-dlp.
4. Keep the original VTT as the canonical transcript artifact.
5. Do **not** require FFmpeg for transcript download, parsing, transcript display, or click-to-seek behavior.
6. Use FFmpeg primarily to merge YouTube's separate video and audio streams without re-encoding.
7. Use FFprobe to verify the final local media file before LinkVault publishes it as playable.
8. Parse VTT in Rust into typed cues for the transcript panel.
9. Render the local video in React and render the selected transcript in a right-side virtualized panel.
10. Clicking or keyboard-activating a cue seeks the video to that cue's start time.
11. Add a video-specific local media protocol that supports byte-range requests. Do not reuse the current newspaper image protocol unchanged.
12. Do not create a fourth provider-local scheduler or queue. Production downloads must use the shared workflow boundary; an internal feature-flagged spike may run one ephemeral download for validation.

The intended experience is:

```text
Paste YouTube URL
→ scan video or playlist
→ select videos, quality, and caption languages
→ download and verify local artifacts
→ open a local player
→ read the synchronized transcript on the right
→ click any transcript cue to jump to that moment
```

---

## 2. Context-drift checkpoint

The original request is narrow:

- support YouTube URLs and playlists through yt-dlp;
- scan uploader-provided and YouTube-generated captions;
- let the user choose caption languages;
- download playable local video;
- display the transcript beside the video;
- let the user click transcript text to seek.

This PRD intentionally does **not** expand V1 into:

- Pluralsight or edX support;
- a general downloader for every yt-dlp website;
- an independent toolchain platform;
- browser-cookie or Google-account authentication;
- AI transcription, Whisper, translation, summarization, or RAG;
- subtitle burning or subtitle embedding;
- channel-wide archiving;
- live-stream recording;
- a cross-provider universal media player;
- arbitrary yt-dlp command arguments.

Future work may build on the foundations, but those features are not V1 acceptance requirements.

---

## 3. Why VTT is the correct transcript source

A WebVTT file is already a timed transcript. It contains text cues with start and end times.

Example:

```vtt
WEBVTT

00:00:02.500 --> 00:00:05.800
Welcome to this course.

00:00:05.800 --> 00:00:09.200
Today we are going to learn React.
```

From this small file, LinkVault can support:

- subtitles over the video through the HTML `<track>` element;
- a transcript panel beside the video;
- current-cue highlighting;
- click-to-seek;
- keyboard navigation;
- future transcript search and timestamped indexing.

No audio extraction or speech-to-text model is required when a suitable caption track already exists.

### Storage decision

V1 stores:

```text
lesson.mp4 or lesson.webm
lesson.en.creator.vtt
lesson.ja.automatic.vtt
linkvault-manifest.json
```

V1 does not create duplicate `.srt`, `.txt`, or normalized `.json` transcript files by default.

The VTT remains the source of truth. Parsed display cues may be cached in memory by file checksum. A future search index may store derived text in SQLite, but it must remain rebuildable from the original VTT.

---

## 4. Component responsibilities

| Component | V1 responsibility |
|---|---|
| `yt-dlp` | Scan YouTube metadata and formats; discover caption tracks; download selected media streams and VTT sidecars. |
| Deno | Execute JavaScript required by current yt-dlp YouTube extraction support. |
| FFmpeg | Merge separate video and audio streams; remux compatible streams when needed. |
| FFprobe | Verify container, codecs, duration, and the presence of playable video/audio streams. |
| Rust provider | Validate URLs, build controlled yt-dlp arguments, own paths, parse VTT, normalize cues, expose typed IPC, and resolve media IDs. |
| React player | Render `<video>`, transcript layout, controls, active-cue state, focus behavior, and click-to-seek interaction. |

### Explicit non-responsibilities

- FFmpeg does not download captions.
- FFmpeg does not power the right-side transcript panel.
- FFmpeg is not required to display an external VTT track.
- React does not parse untrusted paths, build yt-dlp arguments, or decide provider fallback behavior.
- The WebView does not receive arbitrary shell access.

---

## 5. Product goals

- Make a downloaded YouTube video immediately playable inside LinkVault.
- Preserve the chosen caption source and exact language tag.
- Make transcript navigation feel instant and predictable.
- Keep transcript storage negligible compared with video storage.
- Avoid video re-encoding in the default path.
- Keep large-video playback memory-bounded.
- Support offline playback after a completed download.
- Keep the implementation compatible with LinkVault's provider and workflow boundaries.

## 6. V1 non-goals

- Active or scheduled livestreams.
- YouTube channel, handle, feed, history, search, Mix, or subscriptions URLs.
- Private, members-only, or login-required videos.
- Browser cookie import.
- Automatic caption translation.
- Downloading uploader and automatic captions for the same language simultaneously.
- SRT conversion.
- Burned-in subtitles.
- Subtitle embedding into MP4, WebM, or MKV.
- AI transcript cleanup.
- Transcript summarization or semantic search.
- Playback speed persistence across devices.
- Picture-in-picture certification.
- Casting.
- Frame-accurate editing.
- Multiple simultaneous videos.
- Cross-provider player migration.

---

## 7. Supported source inputs

| Input | V1 behavior |
|---|---|
| `youtube.com/watch?v=<id>` | Supported as a single video. |
| `youtu.be/<id>` | Supported as a single video. |
| `youtube.com/shorts/<id>` | Supported as a single video. |
| `youtube.com/playlist?list=<id>` | Supported through explicit playlist scan and item selection. |
| Watch URL containing `list=` | Ask **This video only** or **Entire playlist**; default to **This video only**. |
| Ended livestream replay | Supported only when scan reports an ordinary playable replay. |
| Active or upcoming livestream | Rejected in V1. |
| Channel, handle, feed, search, history, subscriptions, or Mix URL | Rejected before yt-dlp execution. |
| Non-YouTube URL | Rejected; no generic extractor fallback. |

Safety limits:

- maximum 25 pasted root URLs per scan;
- maximum 500 playlist entries returned by a V1 scan;
- maximum 100 selected videos in one submitted workflow;
- sequential media execution by default until native release measurements approve more.

---

## 8. Scan and selection flow

### Single video

```text
Validate URL
→ verify bundled helpers
→ run non-downloading yt-dlp metadata scan
→ sanitize metadata
→ classify media formats
→ classify caption tracks
→ show quality and language options
```

### Playlist

```text
Validate explicit playlist URL
→ run flat playlist scan
→ display playlist entries
→ user selects videos
→ run detailed scans only for selected videos
→ calculate language coverage
→ show common download options
```

The scan response must distinguish:

- uploader-provided captions;
- automatically generated captions.

Translated automatic tracks are hidden in V1.

### Caption selection

The user may choose multiple exact language tags. For each language, V1 supports one policy:

- **Uploader first, automatic fallback**
- **Uploader only**
- **Automatic only**

V1 does not download both origins for the same exact language in one request.

Example:

```text
English (en)       Uploader first, automatic fallback
Japanese (ja)      Automatic only
Chinese (zh-Hant)  Uploader only
```

The backend treats language tags as opaque upstream identifiers. The frontend must not construct regular expressions for `--sub-langs`.

---

## 9. Download behavior

V1 modes:

- **Video and transcripts**
- **Video only**
- **Transcripts only**

Recommended media profiles:

- **Compatible up to 1080p — default**
- Compatible up to 720p
- Best available — advanced, may have limited local playback compatibility

The default profile prioritizes a combination that the LinkVault Windows WebView can play locally, preferably:

```text
H.264 video + AAC audio in MP4
```

If that combination is unavailable, LinkVault must not silently claim compatibility. It should either:

- select a tested WebM combination supported by the installed WebView; or
- warn that the best-available format may require an external player.

Raw yt-dlp format IDs are not stored as the product preference. LinkVault stores a media policy and resolves current format IDs at execution time.

### Media processing

YouTube frequently exposes high-quality video and audio separately:

```text
video-only stream
+
audio-only stream
→ FFmpeg merge/remux
→ final local media file
```

The default pipeline must use stream copy when possible. V1 must not re-encode video merely to satisfy a container preference.

### Caption output

Caption sidecars use deterministic names:

```text
<language-tag>.creator.vtt
<language-tag>.automatic.vtt
```

A selected track must never overwrite another origin or language.

---

## 10. Local player experience

### Layout

Desktop layout:

```text
┌──────────────────────────────────────┬──────────────────────────┐
│                                      │ Transcript               │
│              Video                   │ Language: English        │
│                                      │ Source: Uploader         │
│                                      │                          │
│                                      │ 00:02 Welcome...         │
│                                      │ 00:05 Today we...        │
│                                      │ 00:09 Components...      │
├──────────────────────────────────────┴──────────────────────────┤
│ Lesson title, channel, duration, file information              │
└─────────────────────────────────────────────────────────────────┘
```

Recommended transcript width:

- preferred: 380–460 px;
- minimum: 320 px;
- stack below the player only when the content container cannot maintain a usable video and transcript column.

The video and transcript must remain usable at the application's supported minimum window and at 200% zoom.

### Transcript header

The transcript panel provides:

- selected language;
- source badge: **Uploader** or **Automatic**;
- language switcher when multiple tracks exist;
- **Follow playback** toggle;
- optional **Show captions on video** toggle;
- empty/error state when the selected transcript is unavailable.

### Cue rows

Each cue displays:

- formatted start time;
- normalized cue text;
- active state;
- keyboard-focus state.

Interaction contract:

- mouse click seeks to `cue.startMs`;
- Enter or Space on a focused cue performs the same seek;
- clicking a cue does not force playback;
- if the video was playing, it continues playing after the seek;
- if the video was paused, it remains paused;
- seek targets are clamped to the known media duration;
- cues with invalid timing are not exposed to React.

### Active cue and follow behavior

- The active cue is highlighted while the playback time is within its interval.
- If display cues overlap, the row with the latest start time is the primary active row; other simultaneously active rows may receive a secondary active style.
- Lookup uses binary search or an equivalent indexed method, not a full transcript scan on every frame.
- While playing, the UI may use `requestAnimationFrame`, but React state changes only when the active cue index changes.
- `timeupdate`, `seeked`, `loadedmetadata`, pause, and language changes also reconcile the active cue.
- Auto-follow scrolls only when the active cue leaves the visible transcript viewport.
- Manual transcript scrolling suspends auto-follow temporarily.
- The panel visibly offers **Resume follow** so the UI never fights the user.

### On-video captions

The original VTT may also be attached to the `<video>` element as a `<track>` resource.

On-video captions are separate from the right transcript panel:

- transcript panel may remain visible while overlay captions are hidden;
- overlay captions are optional and off by default when the right panel is open;
- both views use the same selected raw VTT track.

---

## 11. VTT parsing and normalization

VTT is untrusted provider input. Rust owns parsing and normalization because the result changes transcript semantics and future indexing.

Conceptual response:

```typescript
interface YouTubeTranscriptCue {
  id: string;
  startMs: number;
  endMs: number;
  text: string;
}

interface YouTubeTranscript {
  trackId: string;
  videoId: string;
  languageTag: string;
  displayName: string;
  origin: "creator" | "automatic";
  sourceChecksumSha256: string;
  cues: YouTubeTranscriptCue[];
}
```

The parser must handle:

- UTF-8 with or without BOM;
- `WEBVTT` header metadata;
- CRLF and LF line endings;
- cue identifiers;
- millisecond timestamps;
- cue settings such as line, position, size, align, and vertical;
- `NOTE`, `STYLE`, and `REGION` blocks;
- WebVTT voice/class/ruby tags;
- HTML entities;
- multiline cue text;
- overlapping cues;
- right-to-left, CJK, emoji, combining characters, and non-Latin text;
- malformed blocks without crashing the app.

### Automatic-caption normalization

YouTube-generated VTT may contain rolling or overlapping text. A naive text dump can produce:

```text
Hello and welcome
Hello and welcome to today's
welcome to today's lesson
```

V1 should derive display text conservatively:

- remove WebVTT formatting markup;
- decode entities;
- collapse display-only whitespace;
- remove exact duplicate adjacent cues;
- remove only a proven longest suffix/prefix overlap within a narrow timing window;
- preserve intentional repeated words when confidence is low;
- never invent punctuation or rewrite sentences;
- never alter the canonical VTT file.

Normalization must be versioned and fixture-tested. A parser failure must leave the raw VTT intact and produce a typed **Transcript unavailable** error rather than presenting corrupted text.

---

## 12. Large local media delivery

### Adversarial finding

The existing `newspaper-media` protocol is designed for images. It reads the entire resolved file into a `Vec<u8>` and returns `200 OK`.

That implementation must not be reused unchanged for video because:

- a multi-gigabyte video could be loaded into application memory;
- browser seeking depends on byte-range responses;
- returning the entire file for each seek is inefficient;
- the player may fail to seek reliably.

### Required YouTube media protocol

V1 introduces a Rust-owned route such as:

```text
youtube-media://video/<artifact-id>?v=<artifact-version>
youtube-media://caption/<track-id>?v=<track-version>
```

The frontend receives opaque URLs, never absolute filesystem paths.

Video responses must support:

- `GET`;
- `HEAD`;
- `Range: bytes=<start>-<end>`;
- `206 Partial Content`;
- `Content-Range`;
- `Content-Length`;
- `Accept-Ranges: bytes`;
- `Content-Type`;
- stable `ETag`;
- `416 Range Not Satisfiable` for invalid ranges;
- bounded file reads from the requested offset;
- no whole-file buffering.

Caption responses may return the complete small VTT with:

```text
Content-Type: text/vtt; charset=utf-8
```

Native Windows UAT must prove that WebView2 accepts a `<track>` resource from the custom protocol. If it does not, Rust may return the small VTT text through typed IPC and React may create a temporary `text/vtt` Blob URL solely for the optional on-video track. Blob URLs must be revoked on language change and unmount. The transcript panel continues to use Rust-parsed cues in either case.

### Path safety

Rust resolves artifact IDs through provider-owned records and validates:

- the artifact is completed and ready;
- the requested version matches;
- the resolved path remains under the approved YouTube archive root;
- the target is a regular file;
- symlinks and reparse-point escapes are rejected;
- the file size matches the recorded artifact metadata;
- MIME is allowlisted.

The protocol must not accept raw paths or arbitrary file extensions from the frontend.

---

## 13. Persistence and player manifest

Provider-owned records should represent:

### Video artifact

- stable YouTube video ID;
- title and channel metadata;
- duration;
- upload date when available;
- media artifact ID and version;
- relative path;
- MIME/container;
- video codec;
- audio codec;
- width/height;
- byte count;
- checksum;
- completion state.

### Caption track

- stable track ID;
- video ID;
- exact language tag;
- display name;
- origin: creator or automatic;
- relative VTT path;
- byte count;
- checksum;
- track version;
- completion state.

Conceptual player IPC:

```typescript
interface YouTubePlayerManifest {
  videoId: string;
  title: string;
  channelName: string | null;
  durationMs: number | null;
  media: {
    artifactId: string;
    url: string;
    mimeType: string;
    width: number | null;
    height: number | null;
    videoCodec: string | null;
    audioCodec: string | null;
  };
  captions: Array<{
    trackId: string;
    languageTag: string;
    displayName: string;
    origin: "creator" | "automatic";
    vttUrl: string;
  }>;
}
```

Commands:

```text
scan_youtube_source
detail_youtube_scan_items
submit_youtube_download
get_youtube_player_manifest
get_youtube_transcript
cancel_youtube_scan
```

After download submission, generic workflow commands own progress, retry, pause, cancellation, and recovery.

All frontend calls belong behind typed adapters in:

```text
apps/desktop/src/lib/youtube/
```

---

## 14. Proposed module boundaries

Backend:

```text
apps/desktop/src-tauri/src/providers/youtube/
├─ mod.rs
├─ commands.rs
├─ models.rs
├─ url.rs
├─ scan.rs
├─ ytdlp.rs
├─ captions.rs
├─ vtt.rs
├─ formats.rs
├─ planner.rs
├─ executor.rs
├─ media_protocol.rs
├─ repository.rs
└─ errors.rs
```

Frontend:

```text
apps/desktop/src/
├─ components/youtube/
│  ├─ YouTubeView.tsx
│  ├─ YouTubeScanForm.tsx
│  ├─ YouTubeSelectionTable.tsx
│  ├─ YouTubeDownloadOptions.tsx
│  ├─ YouTubePlayer.tsx
│  └─ YouTubeTranscriptPanel.tsx
└─ lib/youtube/
   ├─ ipc.ts
   └─ types.ts
```

Ownership rules:

- React owns DOM/media-element interaction, layout, focus, and transient active-cue presentation.
- Rust owns provider validation, yt-dlp arguments, filesystem paths, artifact state, VTT parsing, normalization, and media-route authorization.
- YouTube must not import LinkedIn or Coursera internals.
- `lib.rs` remains composition only.
- No new React processing loop or provider-local scheduler is permitted.

---

## 15. Bundled-helper integration

V1 bundles fixed, tested Windows binaries:

```text
yt-dlp.exe
deno.exe
ffmpeg.exe
ffprobe.exe
```

This is a direct application dependency, not a user-facing toolchain system.

Requirements:

- add target-specific binaries through Tauri sidecar or resource packaging;
- invoke only from Rust;
- do not expose shell spawn permission to React;
- use argument arrays, never shell command strings;
- ignore user/system yt-dlp configuration;
- disable yt-dlp plugins and remote components;
- use explicit bundled Deno and FFmpeg locations;
- do not call yt-dlp's self-updater;
- update helper versions only through a LinkVault release;
- record exact versions, hashes, licenses, and source/build references in third-party notices;
- terminate yt-dlp and descendant FFmpeg/Deno processes on cancellation or app exit.

No V1 requirements for:

- runtime helper downloads;
- independent helper updates;
- rollback UI;
- user-selected executable paths;
- system `PATH` discovery.

---

## 16. Workflow boundary

ADR-001 prohibits new providers from introducing another scheduler, generic job table, cancellation runtime, or frontend processing loop.

Therefore:

- scan may be ephemeral and cancellable;
- player and VTT parsing can be implemented independently of the download scheduler;
- production download execution must register a YouTube planner and step executors through the shared workflow boundary;
- the minimum acceptable workflow vertical slice is one durable run with resolve, download, merge, verify, publish, progress, cancellation, retry, and restart reconciliation;
- a developer-only feature flag may run one ephemeral download to validate yt-dlp arguments and packaging;
- that spike must not ship as a provider-local production queue.

This is deliberately narrower than completing every future workflow feature, but it avoids creating a fourth legacy engine.

---

## 17. Failure behavior

Stable error classes include:

- invalid or unsupported URL;
- playlist intent required;
- live video unsupported;
- video unavailable/private/login-required;
- helper missing or incompatible;
- yt-dlp extraction changed;
- selected format unavailable;
- selected caption unavailable;
- media merge failed;
- media verification failed;
- VTT malformed;
- transcript normalization failed;
- media artifact missing;
- media range invalid;
- unsupported local codec;
- disk full;
- cancellation;
- publication failed.

User-visible messages must not expose:

- signed media URLs;
- cookies or request headers;
- raw yt-dlp JSON;
- absolute local paths;
- full unredacted stderr.

---

## 18. Adversarial review

### Risk: “The video downloaded, so it will play”

False. The best YouTube format may use a codec/container combination that the LinkVault WebView cannot decode.

Mitigation:

- default to a tested compatible profile;
- verify final codecs with FFprobe;
- test the exact installed Windows WebView;
- show a typed compatibility error before presenting the artifact as playable.

### Risk: reusing the newspaper media route

The existing route reads complete files and lacks video byte-range behavior.

Mitigation:

- create a separate video-capable protocol;
- implement and test `206`, `416`, `HEAD`, and bounded offset reads;
- measure memory while playing and seeking a large file.

### Risk: naive VTT-to-text conversion

Automatic captions can repeat overlapping words, include inline timestamps, and contain markup.

Mitigation:

- preserve raw VTT;
- parse in Rust;
- normalize conservatively;
- maintain creator and automatic fixtures;
- never replace the canonical file with normalized text.

### Risk: custom-protocol VTT is rejected by the native WebView

A custom caption URL may behave differently from an ordinary HTTPS VTT resource because media text tracks have origin and MIME requirements.

Mitigation:

- prove the custom-protocol path in installed Windows UAT;
- return `text/vtt; charset=utf-8`;
- keep a typed-IPC plus temporary Blob URL fallback for the small VTT resource;
- revoke Blob URLs deterministically.

### Risk: active-row highlighting causes render churn

Updating the entire transcript every animation frame will produce jank.

Mitigation:

- virtualize the transcript with the already-installed TanStack virtualizer;
- binary-search cues;
- update React state only when the active cue changes;
- avoid forced scrolling on every playback tick.

### Risk: auto-follow fights manual reading

Automatically centering the active cue after the user scrolls makes the transcript unusable.

Mitigation:

- suspend follow after manual scroll;
- resume only after a clear timeout or explicit **Resume follow** action;
- do not steal focus.

### Risk: cue click unexpectedly starts playback

Users may want to inspect a paused frame.

Mitigation:

- preserve the current play/pause state when seeking.

### Risk: creator and automatic captions collide

yt-dlp behavior can be ambiguous when both origins share the same language tag.

Mitigation:

- choose one origin policy per exact language in V1;
- use origin-specific filenames;
- verify produced artifacts before publication.

### Risk: “no toolchain” is interpreted as “system dependencies”

Requiring users to install yt-dlp, Deno, or FFmpeg would make the feature fragile.

Mitigation:

- bundle fixed helpers directly;
- omit the generic toolchain framework, not the required executables.

### Risk: simplifying creates architecture drift

A provider-local queue would be fast initially but become the fourth lifecycle owner.

Mitigation:

- keep scan/player work independent;
- require the minimal shared workflow vertical slice for production downloads;
- permit only a non-shipping feature-flagged execution spike.

---

## 19. Acceptance criteria

### AC-1 — Caption discovery

Given a video with uploader and automatic captions

When the scan completes

Then LinkVault displays the exact available language tags and identifies each source correctly.

### AC-2 — Multiple language selection

Given several available language tags

When the user selects more than one language with an origin policy for each

Then the submitted request preserves every exact tag and policy without frontend-built regex behavior.

### AC-3 — Raw VTT preservation

Given a selected caption track

When download completes

Then the original VTT is stored unchanged as the canonical caption artifact.

### AC-4 — No FFmpeg subtitle dependency

Given transcript-only mode and a downloadable VTT track

When the workflow runs

Then the transcript completes without invoking FFmpeg.

### AC-5 — Merged playable media

Given separate selected video and audio streams

When download completes

Then FFmpeg merges/remuxes them without video re-encoding and FFprobe verifies one video stream and one audio stream.

### AC-6 — Player manifest

Given a verified completed video

When the player opens

Then React receives an opaque media URL, metadata, and available caption tracks without receiving an absolute filesystem path.

### AC-7 — Byte-range playback

Given a large local video

When the WebView loads, seeks near the end, and performs repeated seeks

Then the protocol returns valid range responses and does not read the entire file into memory.

### AC-8 — Click-to-seek

Given a transcript cue with `startMs = 12500`

When the user clicks it or presses Enter while it is focused

Then the video seeks to approximately 12.5 seconds and preserves its previous play/pause state.

### AC-9 — Active cue

Given normal playback

When the current time crosses cue boundaries

Then the correct transcript row is highlighted without full-list scanning or per-frame React rerendering.

### AC-10 — Manual scroll protection

Given follow mode is enabled

When the user manually scrolls away from the active cue

Then automatic scrolling pauses and the panel offers a clear way to resume following.

### AC-11 — Language switching

Given two completed caption tracks

When the user changes language

Then the right-side transcript and optional video overlay switch to the selected track without restarting media playback.

### AC-12 — Automatic-caption normalization

Given rolling automatic captions

When the transcript is rendered

Then exact adjacent duplication is removed conservatively while the original VTT remains unchanged.

### AC-13 — Malformed VTT safety

Given malformed caption input

When parsing fails

Then the player remains usable, the transcript shows a typed unavailable state, and the application does not crash.

### AC-14 — Path isolation

Given an invalid artifact ID, stale version, symlink, reparse escape, or traversal attempt

When the media protocol resolves it

Then access is denied without exposing a local path.

### AC-15 — Unsupported codec

Given a completed artifact that the installed WebView cannot play

When the player opens

Then LinkVault shows a typed compatibility state and does not falsely mark playback as healthy.

### AC-16 — Process cancellation

Given yt-dlp has spawned FFmpeg or Deno

When the workflow is cancelled or LinkVault exits cooperatively

Then the owned process tree terminates and no descendant continues writing.

### AC-17 — Architecture boundary

Given the implementation diff

When architecture verification and review run

Then YouTube adds no provider-local scheduler, generic queue tables, React processing loop, or imports from another provider's internals.

---

## 20. Verification matrix

### Rust unit and fixture tests

- URL allowlist and rejection matrix.
- Watch URL plus playlist intent.
- Exact caption-language selection.
- Creator/automatic classification.
- Output naming and collision prevention.
- VTT BOM, CRLF, cue identifiers, settings, tags, entities, multiline text.
- `NOTE`, `STYLE`, and `REGION`.
- Automatic rolling-caption overlap.
- Malformed timestamps and partial files.
- RTL, CJK, emoji, and combining characters.
- Media path canonicalization and reparse-point rejection.
- Range parsing: prefix, open-ended, suffix, invalid, and out-of-bounds ranges.
- FFprobe result classification.

### Frontend tests

- two-column and stacked player layout;
- 200% zoom;
- transcript virtualization;
- cue keyboard activation;
- paused and playing seek behavior;
- active-cue changes;
- manual-scroll follow suspension;
- language switching;
- missing transcript;
- unsupported media state;
- no focus theft during playback.

### Native Windows UAT

- clean installed application with no system yt-dlp, Deno, or FFmpeg;
- 720p and 1080p compatible MP4;
- tested WebM fallback;
- uploader captions;
- automatic captions with rolling text;
- playlist selection;
- large file seek near beginning, middle, and end;
- rapid repeated seeks;
- memory measurement during playback;
- sleep/wake;
- app close and cancellation during yt-dlp and FFmpeg;
- Unicode Windows username and output path;
- third-party notices and bundled-helper versions.

---

## 21. Delivery phases

### Phase 0 — specification and dependency spike

- approve this PRD;
- pin candidate helper versions;
- verify current YouTube extraction with bundled Deno;
- prove VTT discovery and download;
- prove compatible MP4 selection;
- record licensing and notices.

### Phase 1 — player and transcript vertical slice

- implement the video-capable local protocol;
- add range tests;
- add Rust VTT parser and fixture corpus;
- add the two-column player;
- add virtualized transcript, highlighting, follow mode, and click-to-seek;
- test with controlled local fixture media.

This phase does not require production YouTube downloading.

### Phase 2 — YouTube scan and selection

- add URL validation;
- add video and flat-playlist scan;
- add detailed selected-item scan;
- add quality and language selection;
- add typed IPC.

### Phase 3 — workflow-backed download

- register YouTube workflow planner and steps;
- download media and VTT;
- merge/remux;
- verify;
- publish;
- progress, cancellation, retry, and restart reconciliation.

### Phase 4 — release hardening

- native Windows compatibility matrix;
- process-tree cancellation proof;
- memory and seek performance;
- security review;
- licensing and notices;
- installed-app UAT.

---

## 22. Recommended PR sequence

```text
PR 0  docs(youtube): define downloader and synchronized transcript player V1
PR 1  feat(media): add range-capable local video protocol
PR 2  feat(youtube): add WebVTT parser and transcript player fixtures
PR 3  feat(youtube): add local player and right-side click-to-seek transcript
PR 4  feat(youtube): add URL, playlist, quality, and caption discovery
PR 5  feat(workflow): add minimal external-process workflow vertical slice
PR 6  feat(youtube): add workflow-backed yt-dlp downloads
PR 7  harden(youtube): certify compatibility, cancellation, security, and release
```

---

## 23. Research references

Primary references checked on 2026-08-20:

- [yt-dlp README](https://github.com/yt-dlp/yt-dlp/blob/master/README.md)
- [Tauri 2 — Embedding External Binaries](https://v2.tauri.app/develop/sidecar/)
- [MDN — `<track>` element](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/track)
- [MDN — WebVTT format](https://developer.mozilla.org/en-US/docs/Web/API/WebVTT_API/Web_Video_Text_Tracks_Format)
- [MDN — `HTMLMediaElement.currentTime`](https://developer.mozilla.org/en-US/docs/Web/API/HTMLMediaElement/currentTime)
- [MDN — HTTP range requests](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Range_requests)
- [MDN — 206 Partial Content](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/206)

The pinned helper versions and installed Windows UAT evidence, not this date-stamped reference section, become the operational compatibility source of truth.
