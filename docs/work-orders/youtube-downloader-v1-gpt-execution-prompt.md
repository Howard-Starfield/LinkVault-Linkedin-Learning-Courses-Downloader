# GPT execution prompt: finish YouTube Downloader V1

Give this file to GPT and instruct it to execute the prompt below. This is an implementation authorization for the named feature branch only, not authorization to merge or release.

## Repository and branch authorization

Use the GitHub plugin to continue and finish the YouTube Downloader V1 implementation directly in this repository:

- Repository: `Howard-Starfield/LinkVault-Linkedin-Learning-Courses-Downloader`
- Authorized branch: `spec/youtube-downloader-v1`
- Expected starting commit when this prompt was committed: `19b6aaff89840cbdf95fa6d615b680598c199790`

You are authorized to inspect, edit, commit, and push changes only to `spec/youtube-downloader-v1`.

Do not modify, merge into, rebase, reset, force-push, delete, or otherwise update `main` or any other branch. Do not create a tag, release, installer publication, or public distribution. Do not open or merge a pull request unless I separately request it.

Use the GitHub plugin for repository inspection and publication. Before making changes:

1. Verify that the repository and branch names exactly match those above.
2. Read the current remote head.
3. Confirm whether it is still the expected starting commit.
4. If the branch has advanced, inspect and preserve every newer change. Never force-push or discard it.
5. Read these files completely before editing:
   - `docs/work-orders/youtube-downloader-v1-handoff.md`
   - `docs/specs/youtube-downloader-v1.md`
   - `docs/architecture/adr-003-youtube-transient-workflow-bridge.md`
   - `docs/architecture/unified-workflow-migration-plan.md`
   - `docs/legal/youtube-v1-approval.md`
   - `docs/third-party/README.md`
   - `docs/third-party/THIRD_PARTY_NOTICES.md`
   - `docs/third-party/youtube-helpers-lock.json`
   - `.github/workflows/youtube-v1-internal-hardening.yml`
   - `apps/desktop/src-tauri/src/workflow/transient/managed_process.rs`
   - `apps/desktop/src-tauri/src/workflow/transient/mod.rs`
   - `apps/desktop/src-tauri/src/app/safe_output_filesystem.rs`
   - `apps/desktop/src-tauri/src/providers/youtube/commands.rs`
   - `apps/desktop/src-tauri/src/providers/youtube/executor.rs`
   - `apps/desktop/src-tauri/src/providers/youtube/scan.rs`
   - `apps/desktop/src/components/youtube/YouTubeView.tsx`
   - `apps/desktop/src/lib/youtube/ipc.ts`
   - `apps/desktop/src/lib/youtube/types.ts`

## Primary objective

Finish the internal YouTube Downloader V1 implementation so that an exact native Windows candidate can safely download public YouTube videos and playlists, uploader or automatic captions, normalized transcript JSON, and merged media using app-managed yt-dlp, Deno, FFmpeg, and FFprobe helpers.

The implementation must be safe and genuinely testable. Do not merely remove the current execution block, fabricate helper metadata, mock native success, or declare the downloader complete based on frontend fixtures.

Do not stop after producing a plan or audit. Implement verified slices, review the diffs, commit them, push them to the authorized branch, inspect the branch-only CI result, and continue with the next safe slice.

## Authorization and product boundary

This work is authorized only for internal implementation and testing under the owner-risk acceptance committed on this branch.

Supported scope:

- Public YouTube videos and explicit public playlists.
- Content the user owns or is authorized to save.
- Video plus transcript, video-only, and transcript-only modes.
- Uploader-provided and automatic captions.
- App-managed, exact, pinned helpers.

Prohibited scope:

- Cookies, browser-profile extraction, or account authentication.
- Private, members-only, paid, age-gated, or otherwise restricted content.
- DRM, access-control, geographic, rate-limit, or platform-restriction bypass.
- Arbitrary yt-dlp arguments, executable paths, plugins, shell commands, or environment overrides.
- Public packaging, distribution, or release.
- Claiming that owner-risk acceptance is legal advice, platform permission, or permission to copy third-party material.

Do not weaken these restrictions to make a test pass.

## Required ownership boundaries

Preserve the repository architecture:

- `workflow::transient` owns process lifecycle, concurrency, state transitions, cancellation, shutdown, and managed helper execution.
- The YouTube provider owns YouTube-domain validation, scan-plan interpretation, transcript selection, yt-dlp argument construction, output planning, normalization, and artifact verification.
- The app safe-filesystem service owns output-root validation, containment, staging, and atomic publication.
- React owns presentation and typed IPC calls only.
- Provider code must not use `std::process::Command`, `tokio::process::Command`, `CreateProcessW`, a shell, or PATH lookup.
- React must never receive a generic shell or arbitrary helper interface.
- Do not introduce a provider-local scheduler or a second durable workflow engine.
- Preserve existing LinkedIn, Coursera, Newspaper, clipping-note durability, updater, and cooperative-exit behavior.

Project-owned TypeScript must not contain explicit `any`, `as any`, `Record<string, any>`, `Promise<any>`, `@ts-ignore`, or `@ts-nocheck`. Use `unknown` plus narrowing. Keep `strict` and `noImplicitAny` enabled.

## Current safety block

The implementation intentionally contains `EXECUTION_HARDENING_COMPLETE = false` in `apps/desktop/src-tauri/src/workflow/transient/managed_process.rs`.

Keep this false until all process-containment, helper-identity, delegated-helper, cancellation, shutdown, and hostile-process tests described below are implemented and passing.

The helper lock is intentionally `status: "unpopulated"`. Do not change it to ready until every required field is populated from authoritative upstream evidence and independently validated. Never invent a version, URL, filename, size, SHA-256 hash, license, source archive, extraction member, compatibility field, or build configuration.

## Implementation sequence

Work through these slices in order. Use focused commits and push each completed, verified slice to the same branch.

### Slice 1: Windows managed-process supervisor

Replace ordinary helper spawning with a workflow-owned Windows implementation that provides:

- Process creation in a suspended state.
- A kill-on-close Windows Job Object.
- Breakaway disabled.
- Assignment to and verification of the Job Object before resuming the process.
- Containment of yt-dlp and delegated Deno/FFmpeg/FFprobe descendants.
- No `cmd.exe`, PowerShell, shell, or PATH resolution.
- One audited Windows argv quoting implementation.
- Concurrent bounded stdout and stderr readers.
- Bounded retained output with explicit truncation reporting.
- Cancellation, timeout, reader failure, assignment failure, and resume failure cleanup.
- Joining readers and waiting for the complete Job/process tree.
- Deterministic cleanup during Cancel, true Quit, and updater restart.
- No orphaned direct child or grandchild.
- Typed errors such as `PROCESS_CONTAINMENT_FAILED` and `HELPER_START_FAILED`.
- No user-controlled executable path.

Add hostile fake-process tests for direct-child/grandchild cancellation, timeout, failure before Job assignment, reader startup failure, resume failure, noisy output, invalid UTF-8 machine output, cancellation/completion races, simultaneous Quit/updater requests, and a dirty clipping note plus an active helper tree.

### Slice 2: Identity-held helper verification

Implement verification that remains valid from validation through process creation and use:

- Resolve helpers only from fixed packaged locations.
- Reject symlinks, junctions, reparse points, device paths, UNC paths, and unexpected targets.
- Open executables with appropriate sharing restrictions.
- Record and verify volume/file identity, exact size, SHA-256, architecture, and final path.
- Hold the verified identity through process creation and its required lifetime.
- Detect replacement attempts between verification and launch.
- Verify yt-dlp, Deno, FFmpeg, FFprobe, yt-dlp EJS, and every other lock-listed loaded asset.
- Verify delegated helpers before yt-dlp receives their paths.
- Pass explicit app-owned absolute paths through controlled argv.
- Remove inherited PATH, config, plugin, cache, and runtime selection.
- Set app-owned TEMP, TMP, cache, Deno, FFmpeg, and FFprobe locations.
- Disable user yt-dlp configuration and plugins.

Add missing, tampered, wrong-architecture, replacement-race, and delegated-helper-substitution tests.

### Slice 3: Helper supply chain

Finish the canonical helper lock and acquisition pipeline using authoritative upstream release information. Prefer official project releases and documentation. Record exact evidence for:

- yt-dlp executable and corresponding source.
- Required yt-dlp EJS support/assets.
- Deno executable and source/license.
- FFmpeg and FFprobe exact Windows build.
- FFmpeg build configuration and corresponding-source obligations.
- Asset URLs and source URLs.
- Exact filenames.
- Asset and source sizes.
- SHA-256 values.
- Archive extraction members.
- Component compatibility.
- SPDX license expressions.
- Required notices and source-offer information.

Requirements:

- No floating `latest` URLs.
- HTTPS only.
- Validate the lock's canonical digest.
- Validate exactly one component for each required role.
- Validate downloaded size and SHA-256 before extraction or promotion.
- Validate the exact extraction member.
- Download into a temporary location and promote only after full validation.
- Never commit downloaded executables, Cargo `target`, frontend `dist`, caches, installers, UAT downloads, or other generated artifacts.
- Confirm the Tauri source-sidecar filename and installed-runtime filename contract with a focused packaging test.
- Keep public packaging blocked even when internal packaging succeeds.

If authoritative hashes or licensing facts cannot be established, keep the lock unpopulated and report the precise missing evidence. Do not guess.

### Slice 4: Workflow correctness

Repair the adversarially identified runtime problems:

- Submission IDs must not become reusable after receipt eviction.
- Implement process-lifetime non-evicting retirement or an equivalent safe design.
- Exact replay must work even if the discovery-plan cache expires or evicts.
- Replay must return the actual current or terminal state, not a hard-coded state.
- Admission must roll back if worker-thread creation fails.
- Cancellation accepted before terminal commit must win over later helper success.
- Terminal states must remain immutable.
- Revisions must remain monotonic.
- A stale run or discovery ID must not cancel different work.
- Shutdown must cancel and join active discovery and download work.
- Active runs must own immutable cloned plans.

Add deterministic tests for each race, replay, shutdown, and stale-identity case. Do not hide the problems by merely increasing cache limits.

### Slice 5: Safe output filesystem

Strengthen the current path-based protections:

- Use stable root, staging, directory, and leaf identities or no-follow handles.
- Prevent root, parent, staging, nested-directory, and leaf junction/reparse swaps.
- Revalidate containment after helper completion and immediately before publication.
- Reject ADS, device names, reserved Windows names, trailing dots/spaces, unsafe components, and path-length overflow.
- Keep staging service-controlled.
- Hash verified contents through the validated identity.
- Flush files and directories where supported.
- Publish a whole completed item directory atomically.
- Never expose a partially completed final directory.
- Never silently overwrite an existing destination.
- Quarantine or clean unsafe staging without following reparse points.

Add hostile swap tests covering root, directory, nested-directory, and leaf replacement.

### Slice 6: Fingerprints, manifests, and reuse

Implement the frozen manifest contract:

- Use the canonical helper-lock digest, not only the yt-dlp executable digest.
- Include provider, schema, occurrence/video/playlist identity, effective mode, format policy, height cap, semantic transcript selection, helper-lock digest, and every artifact hash/size.
- Exclude mutable display metadata, output root, run ID, and scan-plan ID from item artifact identity.
- Revalidate every manifest artifact before reuse.
- Return `skipped_existing` only for an exact compatible verified manifest.
- Return `OUTPUT_COLLISION` for incompatible existing output.
- Implement safe subset reruns and changed run/scan identity with matching item identity.

Add matching, mismatch, tamper, partial-output, and collision tests.

### Slice 7: Scan and source revalidation

Complete bounded discovery:

- Do not accept an unbounded whole-playlist JSON object.
- Consume one bounded machine-readable record per occurrence.
- Reject trailing garbage rather than accepting the first valid line.
- Enforce record, aggregate-output, stderr, entry-count, timeout, and URL limits.
- Preserve duplicate playlist occurrences with distinct opaque occurrence IDs.
- Freeze immutable scan plans.
- Revalidate source/playlist identity and selected ordinals at Start.
- Return `SCAN_PLAN_STALE` for identity or availability changes.
- Report safe metadata drift separately.
- Never pass provider thumbnail URLs directly to React.
- Keep thumbnail placeholders until the bounded Rust-owned thumbnail protocol is implemented and tested.

### Slice 8: Transcripts

Implement:

- Transcript inspection for selected occurrences.
- Uploader and automatic-caption distinction.
- Exclusion of `live_chat`.
- Opaque plan-scoped track keys.
- Deterministic preferred/fallback language selection.
- Exact uploader-over-automatic precedence where specified.
- Missing-transcript warning/failure policy.
- Raw VTT preservation.
- Versioned normalized transcript JSON.
- UTF-8, timestamp, cue, line-ending, entity, formatting-tag, and maximum-size validation.
- No HTML execution or unsafe subtitle-markup rendering.
- Transcript-only mode without unnecessary media download.
- Artifact hashing and manifest binding.

Add fixtures for uploader captions, automatic captions, absent captions, malformed or oversized VTT, duplicate tracks, language fallback, and translated-track ordering.

### Slice 9: Media execution

Implement:

- Video plus transcript, video-only, and transcript-only modes.
- Best, 1080p, 720p, and 480p policies as specified.
- Explicit FFmpeg location.
- Separate-stream merge where needed.
- No silent long re-encode merely to satisfy a preferred container.
- Typed compatibility warnings for VP9, AV1, or nonpreferred containers.
- FFprobe validation of container, expected streams, and duration.
- Conservative free-space preflight where reliable estimates exist.
- Disk-full and merge-failure cleanup.
- Sequential item execution and pause after current item.
- Cancellation during transfer and merge.
- No visible partial final output.

### Slice 10: Mounted native UI

Complete the frontend-native contract:

- Transcript inspection and language/source selection.
- Preferred and fallback languages.
- Video/transcript mode and height policy.
- Continue-without-transcript policy.
- Pause-after-current, resume, and cancel.
- Helper readiness with precise blocked reasons.
- State recovery after route remount or lost Start response.
- Current/latest run recovery.
- Revision ordering and stale-event rejection.
- Duplicate-occurrence selection.
- Truncated-playlist disclosure.
- Accessible live status and keyboard operation.
- Narrow, compact, and wide container layouts.

## Required verification

Run or arrange all relevant verification before declaring completion:

- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --check`
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --lib`
- Complete Rust library tests.
- Focused managed-process and Job Object tests.
- Focused helper-integrity and hostile-replacement tests.
- Focused YouTube-provider and transient-runtime tests.
- Safe-output hostile reparse/TOCTOU tests.
- `npm --prefix apps/desktop run verify:architecture`
- `npm --prefix apps/desktop run verify:ui`
- `npm --prefix apps/desktop run verify:youtube-ui`
- `npm --prefix apps/desktop run verify:no-any`
- `npm --prefix apps/desktop run build`
- Browser verification at narrow, compact, and wide widths.
- Helper-lock verification.
- Existing persistence/repository gates affected by composition-root changes.
- `git diff --check`.
- The `YouTube V1 Internal Hardening` GitHub Actions workflow on every pushed implementation commit.

If the GitHub plugin cannot execute local Windows tests, use the branch-only Windows workflow for automated evidence. Do not change its branch restriction, add publishing permissions, fetch helpers before the lock is authoritative, or misrepresent its source/fixture checks as packaged/native UAT.

Clearly separate:

1. Source inspection.
2. Static or unit-test evidence.
3. Browser fixture evidence.
4. Native development evidence.
5. Installed/package inventory evidence.
6. Exact packaged Windows UAT.

Never claim a stronger category from a weaker one.

## Native UAT completion bar

The internal implementation is not ready for user download testing until an exact native candidate proves:

- No helper installed on PATH is required or selected.
- Exact packaged helper inventory, hashes, versions, paths, and licenses.
- Public single-video scan and transcript discovery.
- Explicit public playlist scan with duplicate-occurrence handling.
- Transcript-only, video-only, and video-plus-transcript downloads.
- FFmpeg merge and FFprobe verification.
- Uploaded and automatic captions.
- Pause/resume.
- Cancellation during transfer and merge.
- Quit and updater restart with no orphaned descendant.
- Output paths containing spaces and Unicode.
- Collision, disk-full, malformed-output, and tampered-helper failure behavior.
- No cookie, account, or restricted-content access.

Use only content I own or am authorized to save for network UAT.

## Independent adversarial review

Before changing `EXECUTION_HARDENING_COMPLETE` to true:

- Perform a fresh adversarial review, preferably with an independent reviewer or subagent if available.
- Review process containment, helper identity, argv/environment control, filesystem TOCTOU, replay/idempotency, cancellation races, shutdown, manifest identity, and legal/release boundaries.
- Address every P0 and P1 finding.
- If any P1 remains, keep execution disabled.
- Record P2 findings and whether they block internal testing.

## Git and publication rules

- Work only on `spec/youtube-downloader-v1`.
- Preserve all existing branch changes.
- Do not force-push.
- Do not merge or rebase `main`.
- Do not alter any other branch.
- Do not commit helper binaries, installers, `target`, `dist`, caches, logs, screenshots, temporary directories, or downloaded UAT media.
- Use focused commits with descriptive messages.
- Push completed slices directly to the same authorized branch.
- Inspect the exact staged payload before every commit.
- Update `docs/work-orders/youtube-downloader-v1-handoff.md` after every meaningful slice.
- Record current commit evidence, not inherited results, when claiming a gate passed.
- Do not create a public release or claim production readiness.
- Do not create or merge a PR unless separately requested.

## Autonomy and stopping conditions

You are authorized to make in-scope branch edits, add tests, run non-destructive validation, commit focused verified changes, and push them to the authorized branch without asking again.

Continue until either:

A. The full internal native completion bar is genuinely satisfied; or

B. Progress is blocked by missing authoritative upstream evidence, unavailable Windows/native execution capability, external credentials, or another condition you cannot safely resolve.

If blocked, exhaust safe in-scope alternatives, keep all safety gates closed, push only verified improvements, and document the exact blocker and next action in the handoff note. Do not mark the overall implementation complete merely because one slice or CI workflow passed.

## Final response

When finished, report:

- Repository and branch.
- Starting and ending commit IDs.
- Every pushed commit and its purpose.
- Major files changed.
- Exact automated checks and results.
- GitHub Actions check names and links.
- Native and installed UAT actually performed.
- Independent-review findings and resolutions.
- Whether `EXECUTION_HARDENING_COMPLETE` remains false or was safely enabled.
- Whether the helper lock remains unpopulated or is authoritative and verified.
- Remaining blockers and next action.
- Confirmation that no other branch, release, binary artifact, or prohibited content path was modified.
