# YouTube Downloader V1 implementation handoff

**Branch:** `spec/youtube-downloader-v1`

**Updated:** 2026-08-20

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

## Current continuation audit

The authorized branch head was confirmed as `7866fe5636b2b6c314276b78c6ff11ed054e3345` before continuation work began; the branch had not advanced beyond the expected handoff commit.

The fresh source audit confirmed the previously recorded enablement blockers are still present and must not be hidden by removing the execution gate:

1. The current managed-process adapter uses an ordinary direct-child spawn and kill path, not suspended `CreateProcessW` plus verified kill-on-close Job Object containment.
2. The current cooperative-exit coordinator resolves renderer durability only; YouTube cleanup happens after exit authorization, and updater installation does not yet enter the same barrier.
3. Helper validation is path/hash based rather than identity-held through process creation and delegated-helper lifetime.
4. Submission receipts are evicted, worker-spawn failure does not roll back admission, and shutdown signals cancellation without joining active work.
5. Playlist discovery accepts one whole JSON object and may accept the first parseable line while ignoring trailing garbage.
6. Output verification remains path based and does not yet close every reparse/TOCTOU window with stable handles.
7. Transcript inspection, normalized transcript JSON, FFprobe verification, exact manifest reuse, and mounted pause/resume/transcript controls remain incomplete.

## Where work stopped

Real helper execution is deliberately disabled in `workflow/transient/managed_process.rs` by `EXECUTION_HARDENING_COMPLETE = false`. The committed helper lock also remains `status: "unpopulated"`. Either condition fails closed, so populating the lock alone cannot accidentally enable execution.

Independent adversarial review found no active P0 while those blocks remain in place. It identified these enablement blockers:

1. Implement a Windows suspended-process supervisor with kill-on-close Job Object containment, descendant cleanup, bounded reader joining, and ordered Quit/updater shutdown.
2. Hold stable executable/file identities from validation through process creation; verify and pin yt-dlp, Deno, FFmpeg, FFprobe, and loaded assets; remove inherited PATH/config/cache/runtime selection.
3. Strengthen output-root, staging, and leaf operations with stable file/volume identities and no-follow handles to close remaining junction/reparse TOCTOU windows.
4. Make submission IDs process-lifetime non-evicting (or durably retired), roll back admission on worker-thread creation failure, and make accepted cancellation win terminal races.
5. Bind manifests/fingerprints to the canonical helper-lock digest and semantic transcript selection, and implement verified `skipped_existing` reuse.
6. Complete transcript inspection/selection, raw VTT plus normalized JSON, source-drift revalidation, FFprobe media verification, and deterministic delegated-helper arguments.
7. Mount native transcript-inspection and pause/resume controls, then add native WebView coverage beyond the browser fixture.
8. Resolve and test the exact Tauri sidecar source filename versus installed runtime filename contract before packaging.

Public release additionally requires the separate `Y-PUBLIC-REVIEW` decision and exact packaged/native UAT evidence.

## Last verified evidence

Evidence inherited from the starting commit:

- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` passed.
- Complete Rust library suite: 583 passed, 0 failed, 4 ignored.
- Architecture, repository UI, YouTube UI, and no-explicit-`any` gates passed.
- Production frontend build passed.
- Browser fixture passed at narrow, compact, and wide widths for scan, occurrence selection, progress, cancellation, and accessibility.
- Helper verification failed closed as intended because authoritative lock metadata is absent.
- `git diff --check` passed.

Continuation evidence:

- The repository and authorized branch names were verified exactly.
- The starting remote head matched `7866fe5636b2b6c314276b78c6ff11ed054e3345` with no newer branch commits to preserve.
- The required handoff, PRD, bridge ADR, migration plan, legal decision, helper-lock/notices, managed-process/runtime, safe-output, provider, and mounted frontend/IPC files were read in full before editing.
- The branch-only Windows internal-hardening workflow has been added; its first run must complete before its commands are recorded here as current-commit evidence.

No packaged/native YouTube download UAT has been claimed.

## Next safe slice

Implement the workflow-owned Windows managed-process supervisor and hostile-process tests under the new branch-only Windows gate. Keep `EXECUTION_HARDENING_COMPLETE` false and the lock unpopulated. Pair process containment with the native cooperative-exit participant so Cancel, true Quit, and updater restart cannot take divergent cleanup paths. Then complete identity-held verification and deterministic delegated-helper arguments before selecting or downloading exact helper assets.

After those gates pass, implement transcript-only execution as the first end-to-end native candidate; media download and FFmpeg merge follow afterward.

The local Rust `target` and frontend `dist` directories used for prior validation were removed before the original handoff because they are regenerable and were not committed. Continuation work must retain the same no-generated-artifacts rule.
