# YouTube Downloader V1 implementation handoff

**Branch:** `spec/youtube-downloader-v1`

**Updated:** 2026-08-20

**Status:** Internal non-executing scaffold; real helper execution, packaging, native UAT, and public release remain blocked.

## Implemented in this branch

- Mounted YouTube route and responsive/accessibility-focused UI.
- Typed frontend IPC for helper status, scan, start, state recovery, and cancellation.
- Rust YouTube provider scaffold with supported-URL validation, bounded machine-output parsing, immutable scan plans, and distinct duplicate-playlist occurrence identities.
- Workflow-owned transient runtime with revisioned state/events and pause, resume, cancellation, and shutdown entry points.
- App-owned staged output service with path/name/reparse checks, artifact hashing, manifest creation, cleanup, and atomic whole-directory publication.
- Pinned-helper lock schema, acquisition/verification scripts, third-party notice scaffold, and an optional helper-enabled Tauri configuration.
- Internal owner-risk acceptance for Y0-Y3 implementation/testing. It does not authorize public packaging, distribution, release, restricted-content access, cookie/account use, or bypass behavior.

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

- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` passed.
- Complete Rust library suite: 583 passed, 0 failed, 4 ignored.
- Architecture, repository UI, YouTube UI, and no-explicit-`any` gates passed.
- Production frontend build passed.
- Browser fixture passed at narrow, compact, and wide widths for scan, occurrence selection, progress, cancellation, and accessibility.
- Helper verification failed closed as intended because authoritative lock metadata is absent.
- `git diff --check` passed.

No packaged/native YouTube download UAT has been claimed.

## Recommended next slice

Start with the workflow-owned Windows managed-process supervisor and its hostile-process tests. Keep `EXECUTION_HARDENING_COMPLETE` false and the lock unpopulated while implementing it. Then complete identity-held verification and deterministic delegated-helper arguments before selecting or downloading exact helper assets. After those gates pass, implement transcript-only execution as the first end-to-end native candidate; media download and FFmpeg merge follow afterward.

The local Rust `target` and frontend `dist` directories used for validation were removed before handoff because they are regenerable and were not committed.
