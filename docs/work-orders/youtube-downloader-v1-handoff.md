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

1. Hold stable executable/file identities from validation through process creation; verify and pin yt-dlp, Deno, FFmpeg, FFprobe, and loaded assets; remove every remaining inherited/config/cache/runtime ambiguity.
2. Strengthen output-root, staging, and leaf operations with stable file/volume identities and no-follow handles to close junction/reparse TOCTOU windows.
3. Bind manifests/fingerprints to the canonical helper-lock digest and semantic transcript selection, and implement verified `skipped_existing` reuse.
4. Complete bounded per-occurrence discovery, source revalidation, transcript inspection/selection, raw VTT plus normalized JSON, FFprobe media verification, and deterministic delegated-helper arguments.
5. Mount native transcript-inspection and pause/resume controls, then add native WebView coverage beyond the browser fixture.
6. Resolve and test the exact Tauri sidecar source filename versus installed runtime filename contract before packaging.

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

No packaged/native YouTube download UAT has been claimed.

## Next safe slice

Push this ordered-exit/workflow slice and require a green branch-only Windows workflow. Then proceed to identity-held verification and deterministic delegated-helper arguments before selecting or downloading exact helper assets.

After those gates pass, transcript-only execution remains the first end-to-end native candidate; media download and FFmpeg merge follow afterward.

Generated Rust `target`, frontend `dist`, helper binaries, installers, caches, logs, screenshots, and UAT downloads are not committed.
