# YouTube Downloader V1 implementation handoff

**Branch:** `spec/youtube-downloader-v1`

**Updated:** 2026-08-21

**Status:** Internal non-executing scaffold under active hardening; real helper execution, packaging, native UAT, and public release remain blocked.

## Implemented in this branch

- Mounted YouTube route and responsive/accessibility-focused UI.
- Typed frontend IPC for helper status, scan, start, state recovery, and cancellation.
- Rust YouTube provider scaffold with supported-URL validation, bounded machine-output parsing, immutable scan plans, and distinct duplicate-playlist occurrence identities.
- Workflow-owned transient runtime with revisioned state/events and pause, resume, cancellation, and shutdown entry points.
- App-owned staged output service with path/name/reparse checks, artifact hashing, manifest creation, cleanup, and atomic whole-directory publication.
- Pinned-helper lock schema, acquisition/verification scripts, third-party notice scaffold, and an optional helper-enabled Tauri configuration.
- Internal owner-risk acceptance for Y0-Y3 implementation/testing. It does not authorize public packaging, distribution, release, restricted-content access, cookie/account use, or bypass behavior.
- Branch-only `.github/workflows/youtube-v1-internal-hardening.yml` automation that runs non-publishing Windows source/fixture gates and explicitly proves the unpopulated helper lock still fails closed. It does not fetch helpers, package an installer, publish artifacts, or exercise network YouTube content.

## Continuation audit

The authorized repository and branch were verified again before the current continuation. The remote branch head was `4e6884f3a73000678af1f8994e84f0f7615f2402` (`docs(youtube): add GPT execution prompt`), one commit ahead of the earlier `19b6aaff89840cbdf95fa6d615b680598c199790` hardening handoff. The newer prompt-only commit was inspected and preserved; no force update, rebase, reset, merge, tag, release, or other branch change was made.

The fresh source audit confirmed the enablement blockers must not be hidden by removing the execution gate:

1. The existing managed-process adapter used an ordinary direct-child spawn and kill path, not suspended `CreateProcessW` plus verified kill-on-close Job Object containment.
2. The cooperative-exit coordinator resolves renderer durability only; YouTube cleanup still happens after exit authorization, and updater installation does not yet enter the same barrier.
3. Helper validation remains path/hash based rather than identity-held through process creation and delegated-helper lifetime.
4. Submission receipts are evicted, worker-spawn failure does not roll back admission, and shutdown signals cancellation without joining active work.
5. Playlist discovery accepts one whole JSON object and may accept the first parseable line while ignoring trailing garbage.
6. Output verification remains path based and does not yet close every reparse/TOCTOU window with stable handles.
7. Transcript inspection, normalized transcript JSON, FFprobe verification, exact manifest reuse, and mounted pause/resume/transcript controls remain incomplete.

## Current slice: Windows process-containment foundation

The current change introduces the first bounded Slice 1 implementation increment while deliberately keeping production helper execution disabled:

- Replaces the production Windows launch path with a workflow-owned native supervisor based on suspended `CreateProcessW`.
- Creates a kill-on-close Job Object, assigns and verifies the suspended process before resume, and sets no breakaway permission.
- Uses an explicit absolute application path, a dedicated Windows argv serializer, an inherited-handle allowlist, empty `PATH`, and a minimal helper environment.
- Drains stdout and stderr through separate bounded readers with explicit truncation reporting.
- Terminates and drains the whole Job on cancellation or timeout, then verifies the Job reports no active processes before returning.
- Adds fail-closed cleanup paths for pre-assignment, reader-startup, and resume failures.
- Adds a feature-gated hostile Windows fixture and focused tests for argv round trips, noisy output, invalid UTF-8, direct-child/grandchild cancellation and timeout, injected startup failures, and cancellation/completion races.
- Extends the branch-only Windows workflow with the focused managed-process test target.

This is a containment foundation, not enablement. `EXECUTION_HARDENING_COMPLETE` remains `false`. The new test-only executable is compiled only with `youtube-process-test`; it is not a packaged helper or release artifact.

## Where work remains blocked

Real helper execution is deliberately disabled in `workflow/transient/managed_process.rs` by `EXECUTION_HARDENING_COMPLETE = false`. The committed helper lock also remains `status: "unpopulated"`. Either condition fails closed, so populating the lock alone cannot accidentally enable execution.

Remaining P1 enablement blockers:

1. Finish the authoritative helper inventory. The compatible yt-dlp `2026.08.19`, bundled EJS `0.8.0`, and Deno `2.9.5` candidates are now recorded from tagged upstream evidence, but FFmpeg/FFprobe extracted identities, build configuration, notices, and corresponding-source/redistribution treatment remain unselected. No version or digest may be inferred or invented.
2. Strengthen output-root, staging, and leaf operations with stable file/volume identities and no-follow handles to close junction/reparse TOCTOU windows.
3. Bind manifests/fingerprints to the canonical helper-lock digest and semantic transcript selection, and implement verified `skipped_existing` reuse.
4. Complete bounded per-occurrence discovery, source revalidation, transcript inspection/selection, raw VTT plus normalized JSON, FFprobe media verification, and deterministic delegated-helper arguments.
5. Replace the native transcript-inspection placeholder with bounded real track parsing/selection, then add native WebView coverage beyond the deterministic browser fixture. Typed inspection and revision-aware pause/resume controls are mounted.
6. Prove the exact Tauri sidecar source filename versus installed runtime filename contract in a helper-enabled package. The source/runtime naming contract is fixed and statically checked, but no helper-enabled package exists yet.

Public release additionally requires the separate `Y-PUBLIC-REVIEW` decision and exact packaged/native UAT evidence.

## Current slice: ordered exit and workflow correctness

The next bounded slice completes the process-lifecycle and transient-runtime blockers without enabling helpers:

- True Quit waits for renderer note durability, requests cancellation of active YouTube discovery or execution, waits for workflow quiescence, joins the worker, and only then authorizes application exit.
- Updater installation enters the same renderer-durability and native-workflow barrier; its Windows pre-exit hook grants the single-use exit authorization only after cleanup.
- Shutdown cancels discovery as well as runs and permanently closes new workflow admission.
- Worker startup is gated until ownership is recorded, and injected spawn failure rolls admission back instead of leaving a ghost running state.
- Accepted cancellation wins over a later executor success, including the final-item race.
- Submission IDs and terminal run snapshots remain retired for process lifetime. Exact replay is checked before scan-plan lookup and returns the real current or terminal revision/state rather than a hard-coded `running` response.
- A Windows-only Newspaper test now compares canonical directory identities instead of textual short-name versus long-name path spellings; this is a test-only correction for the full branch gate.

`EXECUTION_HARDENING_COMPLETE` remains `false`, and no helper lock or executable was changed.

## Current slice: identity-held helper contract

The helper boundary now fails closed around one exact Tauri sidecar contract rather than searching several runtime locations:

- Source-side sidecars must be exactly `binaries/<name>-x86_64-pc-windows-msvc.exe`; Tauri installs them beside the application as `<name>.exe`. The optional helper config is checked against exactly `yt-dlp`, `deno`, `ffmpeg`, and `ffprobe` in that order.
- Rust and Node now compute the same recursively key-sorted canonical helper-lock digest. Runtime item fingerprints receive the complete lock digest rather than only the yt-dlp executable digest.
- Every ready-lock component must have one exact target-triple filename, positive locked size, lowercase SHA-256, and one matching installed runtime filename.
- Each installed executable is opened through a non-reparse regular-file handle that denies write/delete sharing, then validated for locked size, SHA-256, x86-64 PE structure, volume identity, and file identity.
- All four handles remain held across process creation and the complete yt-dlp/delegated-helper lifetime. They are revalidated after suspended `CreateProcessW` and before resume.
- yt-dlp receives app-owned absolute Deno and FFmpeg locations. `PATH` stays empty; yt-dlp config, plugins, updates, and cache are disabled; Deno/home/cache/app-data locations are app-owned temporary directories.
- Hostile Windows coverage now proves wrong-digest rejection and that write/rename replacement is denied while the verified handle is held.
- The branch workflow now treats the intentionally nonzero unpopulated-lock verifier as a passing fail-closed assertion under current PowerShell behavior.

The authoritative lock is still deliberately unpopulated. Current upstream evidence confirms yt-dlp distributes signed checksum assets, Deno distributes per-target checksum assets, and yt-dlp requires a compatible EJS package/version; FFmpeg itself links to third-party Windows builders. Exact compatible selections and corresponding-source/license records are the next supply-chain decision, not a value to guess in code.

## Current slice: mounted transcript inspection and run controls

The next bounded UI/runtime contract is complete without enabling helpers:

- TypeScript now mirrors the existing native transcript occurrence/track contract and revision-aware run mutation request.
- The IPC adapter invokes the registered native transcript inspection, pause, and resume commands with typed responses.
- The mounted YouTube view can inspect selected occurrences and render reported language/source/format metadata. The current native provider still returns an honest empty-track placeholder until bounded real extraction is implemented.
- Pause requests are revision-guarded, finish the current item, and settle paused before the next occurrence; resume is also revision-guarded. Deterministic preview behavior matches those transitions and rejects stale mutations.
- Browser coverage now exercises transcript metadata plus pause/resume/cancel behavior at narrow, compact, and wide widths.
- The helper research note freezes tagged yt-dlp/EJS/Deno candidate evidence but deliberately leaves the lock unpopulated while FFmpeg/FFprobe redistribution and extracted-file identity remain unresolved.

This slice does not download a helper, parse live YouTube subtitle metadata, launch a native helper, populate the lock, package an installer, or claim native WebView/download UAT.

## Verification evidence

Evidence inherited from the earlier scaffold commit:

- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` passed.
- Complete Rust library suite: 583 passed, 0 failed, 4 ignored.
- Architecture, repository UI, YouTube UI, and no-explicit-`any` gates passed.
- Production frontend build passed.
- Browser fixture passed at narrow, compact, and wide widths for scan, occurrence selection, progress, cancellation, and accessibility.
- Helper verification failed closed as intended because authoritative lock metadata is absent.
- `git diff --check` passed.

Current-slice evidence at authoring time:

- Repository, branch, prompt commit, and parent relationship were verified through the GitHub connector.
- The required prompt, handoff, PRD, bridge ADR, migration plan, legal decision, helper-lock/notices, managed-process/runtime, safe-output, provider, and mounted frontend/IPC files were inspected before editing.
- Source review confirms the execution constant is still `false` and the helper lock is still unpopulated.
- The initial `694cae6` workflow run failed at Rust formatting before compilation. The formatting and three resulting `windows-sys` ABI/import compile findings were corrected without changing the supervisor design.
- Current local Windows evidence after that repair: Rust formatting passed; `cargo check --lib` passed; the complete Rust library suite passed with 584 passed, 0 failed and 4 ignored; and all 7 focused managed-process hostile-fixture tests passed.
- Architecture, repository UI, YouTube UI, no-explicit-`any`, production frontend build and `git diff --check` passed on the repaired tree.
- Helper verification still fails closed as required while the authoritative lock is unpopulated.
- The repaired commit still requires a green branch-only `YouTube V1 Internal Hardening` workflow before this slice is closed as remote CI evidence.
- Workflow run `32541987104` proved formatting and library compilation, then exposed one unrelated full-suite Windows assertion comparing an 8.3 temp path spelling with its canonical spelling. The resolved directory identity matched; the current slice corrects only that assertion and requires a new full workflow run.
- Current lifecycle/runtime evidence: Rust formatting and `cargo check --lib` passed; the complete library suite passed with 590 passed, 0 failed, and 4 ignored; all 7 hostile managed-process tests passed; and the focused transient-runtime tests cover shutdown/join, discovery cancellation, worker-spawn rollback, process-lifetime submission retirement, retained terminal replay, and accepted-cancellation precedence.
- Architecture, repository UI, YouTube UI, no-explicit-`any`, frontend production build, and `git diff --check` passed. Helper verification still fails closed exactly because the authoritative lock remains unpopulated.
- Lifecycle commit workflow `32542986222` passed the full Rust suite, hostile managed-process suite, architecture, repository UI, YouTube UI, no-explicit-`any`, and frontend build. Its only red step was the expected helper-verifier failure being promoted to a terminating PowerShell error before the assertion wrapper ran; the current workflow correction captures that exit code explicitly.
- Current identity-slice evidence: Rust formatting and `cargo check --lib` passed; the complete library suite passed with 593 passed, 0 failed, and 4 ignored; all 8 hostile managed-process tests passed, including handle-held tamper/replacement denial; architecture, repository UI, YouTube UI, no-explicit-`any`, frontend production build, JS syntax checks, fail-closed helper verification, and `git diff --check` passed.
- Current mounted-controls evidence: Rust formatting and `cargo check --lib` passed; the complete Rust library suite passed with 595 passed, 0 failed, and 4 ignored; the focused pause-after-current/stale-revision/resume tests include withdrawing a pause request before the current item finishes; architecture, repository UI, YouTube UI, no-explicit-`any`, TypeScript production build, and `git diff --check` passed. The real browser fixture passed transcript inspection, revision-aware pause/resume, cancellation, accessibility, and no-horizontal-loss checks at narrow, compact, and wide widths.

No packaged/native YouTube download UAT has been claimed.

## Current stopping point and next safe slice

The bounded filesystem/contract slice is implemented locally on this branch:

- output-root and managed staging ancestors are held by stable no-follow directory handles;
- attempt creation is guard-chained and cleanup is limited to direct regular files in the exact service-owned attempt shape;
- Windows publication uses a no-replace rename of the held attempt handle and immediately rechecks published names and hashes;
- scan parsing rejects trailing machine-output garbage, playlist discovery is bounded one item past the visible limit, and plans carry an order-sensitive source snapshot digest;
- canonical artifact fingerprints/manifest projection and deterministic semantic transcript selection are implemented as pure contracts;
- a bounded WebVTT normalizer emits deterministic versioned transcript JSON; and
- the helper-lock scripts distinguish non-executable evidence from a ready lock and keep the authoritative unpopulated lock fail-closed.

This is not native downloader completion. Execution remains disabled and no helper package, live YouTube download, FFprobe verification, normalized transcript executor wiring, exact `skipped_existing` reuse, partial import, or packaged/native UAT is claimed.

The next safe slice is transcript-only fixture execution: wire verified transcript inspection/selection and the WebVTT normalizer into the executor, publish raw VTT plus normalized JSON through the proven attempt capability, then add exact manifest reuse. After that, select and document exact FFmpeg/FFprobe distribution identities and implement FFprobe media validation before any Y3 media candidate. Keep `EXECUTION_HARDENING_COMPLETE = false` and the helper lock non-ready until every helper identity/license/source and native containment gate passes.

Generated Rust `target`, frontend `dist`, helper binaries, installers, caches, logs, screenshots, and UAT downloads are not committed.

## 2026-08-23 completion continuation (supersedes the stopping point above)

The bounded Y1-Y3 implementation is now integrated for internal testing:

- occurrence-bound transcript inspection and deterministic semantic selection;
- raw WebVTT preservation plus deterministic normalized transcript JSON;
- exact strict-manifest reuse with collision-on-mismatch behavior;
- FFprobe-based bounded media validation and compatibility warnings;
- safe-output leases held across verification and atomic publication;
- fail-closed public-availability admission plus selected-source identity,
  ordinal and availability revalidation immediately before run construction;
- exact yt-dlp, Deno, FFmpeg and FFprobe supply-chain records under one ready
  lock digest; and
- mounted UI controls for ambiguous playlist URLs and transcript languages.

Completion evidence for this continuation:

- the production managed-process path verified all four packaged executable
  bytes, launched yt-dlp `2026.08.19`, and launched pinned FFprobe successfully;
- the release LinkVault executable and an NSIS internal bundle were produced;
- updater signing then failed closed because `TAURI_SIGNING_PRIVATE_KEY` was not
  provided; no signing check was disabled or bypassed;
- an adversarial Luna audit found and drove closure of unsafe reuse, unknown
  availability, stale-source admission, unbounded VTT reading, unheld future
  companion assets, and transcript-label binding gaps; product `Video only`
  correctly remains playable video with an audio stream and no transcript;
- Rust formatting/checking passed; the library suite passed 649/649 with four
  expected ignored tests; and all eight hostile Windows process tests passed;
- helper-lock schema and offline inventory verification, no-explicit-`any`,
  frontend build, static YouTube UI checks, and the narrow/compact/wide browser
  fixture passed; and
- helper fetching now removes verified acquisition archives/source tarballs and
  atomically promotes only the four reviewed executables.

Generated helper and package artifacts are removed before commit. No live
content download, signed installer, or public-release approval is claimed. The
next manual step is native UAT with a public source the operator owns or is
expressly authorized to save; record that authorization outside the repository.
