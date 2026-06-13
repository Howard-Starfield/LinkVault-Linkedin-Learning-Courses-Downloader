# LinkedIn and Coursera Downloader Audit

Date: 2026-06-13

## Purpose

We recently added an isolated Coursera downloader tab to LinkVault while keeping the original LinkedIn Learning downloader as the main app path. This document captures what is currently verified, what appears broken or incomplete, and what we should discuss before the next implementation pass.

The main questions:

1. Why does the LinkedIn download button appear unavailable after pasting a course URL?
2. Does the new Coursera downloader actually work like the reference `coursera-dl-master` implementation?
3. What UI and backend improvements should be prioritized next?

## Current Repo Map

- App root: `apps/desktop`
- Main LinkedIn UI: `apps/desktop/src/App.tsx`
- LinkedIn Rust command/backend surface: `apps/desktop/src-tauri/src/commands.rs`
- LinkedIn parser/backend helper: `apps/desktop/src-tauri/src/linkedin.rs`
- Coursera UI: `apps/desktop/src/components/coursera/CourseraView.tsx`
- Coursera IPC wrappers: `apps/desktop/src/lib/coursera/ipc.ts`
- Coursera Rust command surface: `apps/desktop/src-tauri/src/coursera/commands.rs`
- Coursera Rust implementation modules: `apps/desktop/src-tauri/src/coursera/*`
- Python reference implementation: `coursera-dl-master/coursera/*`
- Existing Coursera implementation plan: `docs/coursera-tab-implementation.md`

## Verification Run

Commands run from repo root:

```powershell
npm run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Results:

- Frontend TypeScript/Vite build passes.
- Rust tests pass: 266 passed, 0 failed, 2 ignored.
- The old `pnpm --dir apps/desktop build` command failed because Windows PowerShell blocked `pnpm.ps1`; root scripts now use npm.
- These tests do not prove real Coursera downloading. The live Coursera auth tests are ignored, so a real enrolled course plus CAUTH token is still required before we can call the Coursera path fully verified.

## Fresh Coursera Research

Checked on 2026-06-13:

- Coursera notified `coursera-dl` that `onDemandCourseMaterials.v1` was being deprecated and that `onDemandCourseMaterials.v2` should be used instead: <https://github.com/coursera-dl/coursera-dl/issues/834>.
- The maintained/forked downloader ecosystem still describes the same core model: use an enrolled Coursera account, fetch course materials, then download videos/resources into named folders. `cs-dlp` documents support for one or more class names, regex filters, file-format filters, and CAUTH-style access: <https://github.com/Superoldman96/cs-dlp>.
- A newer GUI downloader exists, but its current open issues are useful warning signs for our implementation: browser auth can fail, manual CAUTH input is requested, subtitle language persistence can fail, some courses fail, and graded assignments may not download: <https://github.com/touhid314/Coursera-Downloader/issues>.
- Coursera's learner support still exposes official manual video downloads through the web player Downloads section, but not a public consumer bulk-download API: <https://www.coursera.support/s/article/learner-000001476>.
- Coursera's public developer portal is aimed at Business/Campus/Government integrations, not learner bulk-download tooling: <https://dev.coursera.com/get-started>.

Planning implications:

- Treat `onDemandCourseMaterials.v2` as the baseline syllabus/materials endpoint.
- Prefer CAUTH/browser-session based auth first; keep email/password as lower priority because modern Coursera login flows are more likely to be guarded.
- Build clear failure states for browser auth and manual CAUTH entry.
- Do not promise full graded assignment parity in the first working slice.
- Add live manual test documentation because endpoint behavior is drift-prone.

## Execution Plan And Log

Status legend: `Planned`, `In progress`, `Complete`, `Blocked`.

| Step | Status | Notes |
| --- | --- | --- |
| 1. LinkedIn Start button click-through validation | Complete | Button should enable after URL input. On click, missing folder opens the picker; missing `li_at` prompts and focuses the token field. |
| 2. LinkedIn multi-link verification | Complete | Rust parser supports newline order and 105 space-separated URLs; full Rust suite was rerun after edits. |
| 3. Coursera endpoint research | Complete | `v2` materials endpoint and CAUTH/browser auth remain the practical baseline. See research notes above. |
| 4. Coursera command honesty and persistence | Complete | DB schema, preferences, jobs, events, history, retry, clear failed, and folder lookup now use SQLite. |
| 5. Coursera first real download path | In progress | CAUTH loading, v2 syllabus fetch, parser, orchestrator execution, streaming downloads, and event persistence are wired. Live verification still needs a real enrolled course/session. Non-http Coursera asset references are currently skipped with explicit events. |
| 6. Re-download guard | Complete | LinkedIn and Coursera Start flows ask before re-queuing courses/classes already present in completed local history. Coursera also has a backend duplicate guard with explicit `forceRedownload`. |
| 7. UI improvements | Planned | Readiness indicators, clearer auth state, selected-class rows/chips, advanced options collapse. |
| 8. Verification and manual live-test checklist | In progress | `npm run build` passes via `npm.cmd` in PowerShell. `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml` passes with 266 passed, 0 failed, 2 ignored. Manual Coursera live checklist is documented but not yet executed. |

## Implementation Pass 1 Completed

Code changes completed:

- LinkedIn Start Download is enabled after URL input rather than requiring folder and token first.
- LinkedIn Start Download now asks for a folder with the native picker if no folder is set.
- LinkedIn Start Download prompts and focuses the `li_at` input if no saved session/token exists.
- LinkedIn Start Download checks completed local history and asks before re-downloading a course already recorded as complete.
- Coursera Start Download checks completed Coursera history and asks before re-downloading a class already recorded as complete.
- Shared SQLite initialization now creates `coursera_jobs`, `coursera_job_events`, and `coursera_settings`.
- Coursera command bootstrap now reads real persisted jobs, events, preferences, token state, and completed history.
- Coursera queue creation now persists jobs and queue events.
- Coursera retry, clear failed, history, and folder lookup now use SQLite.
- Coursera processing no longer pretends success with zero artifacts; queued jobs now load saved CAUTH, fetch the v2 syllabus, run the orchestrator, persist emitted events, and update job status/counts.
- Coursera downloads now stream to `.tmp` files and atomically rename on success instead of buffering whole videos in memory.
- Coursera non-http asset references are skipped explicitly until the asset resolver is implemented.
- Coursera backend queue creation rejects locally completed classes unless the UI/user sends `forceRedownload`; confirmed re-download queue events include that flag.

Verification:

```powershell
npm run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Both passed after the latest pass.

## Detailed Coursera Implementation Plan

Goal: implement the Coursera side as a desktop-app-native port of the useful `coursera-dl-master` behavior, while staying honest about modern endpoint drift and starting with CAUTH/browser-session access.

### Phase C1: Real Processing Spine

Status: partially complete.

Files:

- `apps/desktop/src-tauri/src/coursera/commands.rs`
- `apps/desktop/src-tauri/src/coursera/orchestrator.rs`
- `apps/desktop/src-tauri/src/coursera/job.rs`
- `apps/desktop/src-tauri/src/coursera/auth.rs`

Work:

1. Complete: Replace the `not_implemented` processing path with real work.
2. Complete: Load the next queued job from `coursera_jobs`.
3. Complete: Deserialize `options_json` into `CourseraOptions`.
4. Complete: Load saved CAUTH from `linkvault.coursera.dpapi`.
5. Complete: Build an authenticated Coursera client.
6. Complete: Fetch syllabus through `onDemandCourseMaterials.v2`.
7. Complete: Parse modules with the existing parser.
8. Complete: Run `CourseraDownloader::download_modules`.
9. Complete: Persist each emitted event to `coursera_job_events`.
10. Complete: Update job status and counts.
11. Remaining: Live-test with a real enrolled course.
12. Remaining: Resolve non-http Coursera asset references instead of skipping them.

Acceptance:

- One queued Coursera job changes to `Completed`, `Failed`, or `Cancelled` based on real work.
- UI history and activity panels show persisted events after app restart.
- No fake success path remains.

### Phase C2: Streaming Downloader And Resume

Files:

- `apps/desktop/src-tauri/src/coursera/downloader.rs`
- `apps/desktop/src-tauri/src/coursera/orchestrator.rs`

Work:

1. Complete: Replace `resp.bytes()` whole-response buffering with streaming writes.
2. Complete: Emit progress events during download.
3. Complete: Write to `.tmp` files and atomically rename on success.
4. Complete: Add cancellation checks between chunks.
5. Remaining: Implement `resume` with `Range` requests when partial files exist.
6. Remaining: Harden `overwrite` and existing-file skip behavior with tests.

Acceptance:

- Large videos do not load fully into memory.
- Progress events are visible/persisted.
- Resume works after an interrupted download.

### Phase C3: Resource Parity

Files:

- `apps/desktop/src-tauri/src/coursera/extractors/*`
- `apps/desktop/src-tauri/src/coursera/filter.rs`
- `apps/desktop/src-tauri/src/coursera/format.rs`

Work:

1. Validate lecture video/subtitle extraction against real or recorded fixtures.
2. Validate supplements/assets/resources.
3. Keep quiz/exam HTML export.
4. Decide whether notebooks/programming assignments are v1 supported or explicitly deferred.
5. Implement playlist generation if `generatePlaylists` is enabled.
6. Make skipped and failed URL reporting match the Python reference model.

Acceptance:

- Fixture tests cover video, subtitle, PDF/resource, quiz HTML, and skipped URL cases.
- UI counters distinguish completed, failed, skipped, and cancelled artifacts.

### Phase C4: Re-download Guard Hardening

Status: complete for the current guardrail; in-app dialog polish remains a UX follow-up.

Files:

- `apps/desktop/src/components/coursera/CourseraView.tsx`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src-tauri/src/coursera/job.rs`
- `apps/desktop/src-tauri/src/cache.rs`

Work:

1. Complete: Keep the current UI confirmation.
2. Complete: Add backend duplicate detection so accidental direct command calls cannot silently duplicate completed work.
3. Complete: Add an explicit `forceRedownload` request field if the user confirms.
4. Complete: Record confirmed re-download decisions as queue events.
5. Remaining UX polish: move from native `window.confirm` into a designed in-app dialog if desired.

Acceptance:

- Previously completed courses/classes are not re-queued unless the user confirms.
- Confirmed re-downloads are auditable in the event log.

### Phase C5: UX Completion

Files:

- `apps/desktop/src/App.tsx`
- `apps/desktop/src/components/coursera/CourseraView.tsx`
- `apps/desktop/src/index.css`

Work:

1. Add readiness indicators for URL/class input, folder, and auth.
2. Show selected Coursera classes as removable rows/chips.
3. Keep advanced Coursera options collapsed by default.
4. Add clear CAUTH/browser-auth status and manual CAUTH fallback guidance.
5. Add "Preview syllabus" as a first-class preflight action.

Acceptance:

- Start buttons feel actionable, not mysteriously disabled.
- Missing requirements are explained at click time.
- Coursera setup is understandable before a long download begins.

### Phase C6: Live Manual Test Checklist

Status: documented, not executed.

Work:

1. Document required test account state: enrolled course, accepted honor code, downloadable lecture.
2. Document CAUTH capture steps.
3. Run ignored live tests with env vars or manual CAUTH.
4. Test one video/subtitle/resource download.
5. Test cancellation.
6. Test retry/resume.
7. Test re-download confirmation.

Acceptance:

- A future agent or user can verify Coursera behavior without rediscovering the full auth/download path.

Manual checklist:

1. Use a Coursera account enrolled in a course that has at least one downloadable lecture video and one subtitle track.
2. Open the course in the browser once and accept any honor-code, enrollment, or course-access prompts.
3. Capture the `CAUTH` cookie from the logged-in Coursera browser session.
4. Paste/save the CAUTH token in the Coursera tab.
5. Select a small empty download folder.
6. Enter one course URL in the form `https://www.coursera.org/learn/<class-slug>`.
7. Use Preview Syllabus first; confirm the v2 syllabus endpoint returns modules and lessons.
8. Start the download with videos and subtitles enabled, quizzes/resources optional.
9. Confirm a queued job becomes active, emits events, and finishes as completed or failed with a specific error.
10. Restart the app and confirm the job/history/events are still visible from SQLite.
11. Start the same course again and confirm the re-download prompt appears before re-queueing.
12. Test cancellation on a larger file and confirm the job becomes cancelled or failed without pretending success.
13. Retry a failed/cancelled job and confirm events continue to append under the job.

## Finding 1: LinkedIn Start Button

The LinkedIn start button is gated by `canStart` in `apps/desktop/src/App.tsx:405` and rendered disabled at `apps/desktop/src/App.tsx:996`.

Previous condition:

```ts
courseUrls.trim().length > 0 &&
folder.trim().length > 0 &&
(token.trim().length > 0 || hasSavedToken) &&
!isProcessingDownload
```

That meant pasting only a course URL was not enough to enable the button. The button also needed:

- A download folder.
- Either a pasted LinkedIn `li_at` token or a saved token loaded by bootstrap.
- No active processing state.

Updated behavior:

- URL text now enables the Start Download button.
- If the folder is missing, clicking Start Download asks for it with the native folder picker.
- If the LinkedIn session is missing, clicking Start Download prompts for `li_at` and focuses the token field.
- Processing/validation states still disable the button while work is active.

Likely causes for the reported symptom:

- The UI does not explain why the button is disabled. If the user only pasted the URL, the disabled state is expected but unclear.
- If a saved token exists but `hasSavedToken` is false after bootstrap, the button stays disabled until a token is pasted again.
- If a previous run leaves `isProcessingDownload` true in the UI, the button remains disabled until the component state resets.
- Less likely from code inspection: Coursera URL parsing directly broke LinkedIn parsing. The LinkedIn parser is still called through `parse_linkedin_course_urls`, and the `canStart` condition does not inspect parsed URL state.

Discussion options:

1. Keep the current hard gate, but add a visible disabled-reason helper near the button.
2. Enable the button after URL input and validate missing folder/token on click with targeted toasts.
3. Add a compact readiness checklist: URL, folder, session.
4. Add a "Use saved session" indicator that shows whether bootstrap actually found the saved LinkedIn token.

Recommended direction: option 2 plus a small session indicator. It feels less broken because the primary action remains clickable, and the app can tell the user exactly what is missing.

## Finding 2: Coursera Is Wired But Not Fully Equivalent Yet

The Coursera tab now has a real UI, typed IPC wrappers, SQLite jobs/events/settings, Rust config/types, parsers, extractors, and a Tauri queue processing path that calls the Coursera downloader. It is no longer a scaffold queue, but it is still not fully equivalent to `coursera-dl-master` until live CAUTH testing and remaining resource parity work are complete.

Current evidence:

- `apps/desktop/src/components/coursera/CourseraView.tsx:309` starts a Coursera run, calls `startCourseraDownloadJobs`, then calls `processQueuedCourseraBatch`.
- `apps/desktop/src-tauri/src/coursera/commands.rs` persists queued jobs, loads saved CAUTH, fetches the v2 syllabus, calls `CourseraDownloader::download_modules`, stores emitted events, and updates job status/counts.
- `apps/desktop/src-tauri/src/coursera/downloader.rs` streams downloads to `.tmp` files with progress callbacks.
- `apps/desktop/src-tauri/src/coursera/job.rs` now supports persisted history, recent events, retry, clear failed/cancelled, and completed-job lookup.
- `apps/desktop/src-tauri/src/coursera/orchestrator.rs` explicitly skips non-http resource links until Coursera asset resolution is implemented.

Conclusion: the Coursera feature is now a real first-slice downloader path, but it remains partially verified. The biggest risk is live endpoint/auth drift, followed by supplement/asset parity and resume behavior.

## Reference Behavior To Match

The Python reference flow in `coursera-dl-master/coursera/coursera_dl.py` and `coursera-dl-master/coursera/workflow.py` does the following:

- Logs in or accepts a CAUTH cookie.
- Expands specializations when requested.
- Fetches/parses syllabus modules.
- Builds the course/module/section/lecture directory structure.
- Filters sections, lectures, resources, formats, and ignored formats.
- Downloads resources through consecutive or parallel downloaders.
- Supports resume/overwrite/skip-download behavior.
- Writes in-memory quiz/exam HTML artifacts.
- Creates playlists when requested.
- Tracks skipped and failed URLs.
- Reports likely course completion based on file timestamps.

The Rust Coursera implementation has partial equivalents for many of these pieces, but not the full end-to-end path.

## Coursera Parity Gaps

Highest priority gaps:

- Live-test `process_queued_coursera_batch` with a real enrolled course and saved CAUTH.
- Resolve Coursera `asset://` or other non-http references into downloadable URLs where possible.
- Add fixture or mocked end-to-end tests that prove a queued Coursera job can become completed without live Coursera.
- Add backend duplicate detection with an explicit `forceRedownload` request field.
- Improve artifact-level counts so completed/failed/skipped reflect files, not only job-level outcomes.

Downloader gaps:

- Range-request resume is still not implemented.
- Existing-file skip/overwrite behavior needs more tests in the live Coursera path.
- Parallel job support is represented in options but not implemented in the orchestrator.
- External downloader support such as `curl`, `wget`, or `aria2c` is not implemented.

Feature gaps vs Python reference:

- Specialization expansion returns the input unchanged.
- List enrolled courses returns an empty list.
- Notebook extraction returns empty in v1.
- Programming/supplement/resource extraction looks partial and should be tested against real Coursera fixtures.
- Playlist generation is represented by an option but not wired.
- Completion heuristics and skipped/failed URL reports are not equivalent yet.
- Live Coursera integration tests are ignored and need a documented manual test path with real credentials or CAUTH.

## UI Improvement Ideas

LinkedIn tab:

- Make the Start Download button clickable once URL text exists, then show exact missing requirements on click.
- Add a readiness row: URL parsed, folder selected, session ready.
- Show a clearer saved-token state and a "refresh saved session" action.
- Separate URL validation from download start so paste feedback does not feel like the button is broken.
- Add "Validate URLs" as a secondary action if we want explicit preflight.

Coursera tab:

- Split setup into three clear zones: Course input, Auth, Output/options.
- Show selected classes as removable chips or table rows after parse.
- Add a "Preview syllabus" button per selected class, because that command is closer to real functionality than full download.
- Surface CAUTH vs email/password vs saved token status without requiring the user to infer it from disabled buttons.
- Keep advanced options collapsed by default: filters, formats, resume/overwrite, playlists, parallelism.

## Backend Improvement Ideas

Short term:

- Add a `linkedinStartDisabledReason()` helper in React and tests for each missing state.
- Add unit tests for the LinkedIn button readiness logic.
- Add a mocked Coursera end-to-end processing test around the command/orchestrator boundary.
- Execute the documented manual live-test checklist for CAUTH, one enrolled class, cancellation, and retry.

Medium term:

- Harden the real Coursera process loop:
  1. Add fixture-driven coverage.
  2. Improve artifact counts.
  3. Resolve asset links.
  4. Implement resume/range requests.
  5. Add playlist generation.
- Implement resume with range requests and temp file handling.
- Add fixture-driven tests comparing Rust parser/extractor output to the Python reference fixtures.
- Add one ignored live integration test script with required environment variables documented.

Long term:

- Decide whether the Rust port should match all `coursera-dl` CLI options or only the subset useful in a desktop app.
- Add a shared job engine abstraction only if LinkedIn and Coursera queues begin duplicating meaningful logic.
- Add import/export of job diagnostics for user support.
- Add an "open logs" or "copy diagnostic summary" action for failed downloads.

## Suggested Next Discussion

Resolved in Pass 1:

1. LinkedIn Start Download now becomes actionable after URL input.
2. Missing folder/session requirements are handled at click time.
3. Coursera uses separate `coursera_*` tables, but through the shared DB initializer.
4. Completed local history now guards against accidental re-downloads in both tabs.
5. Coursera processing now loads saved CAUTH, fetches the v2 syllabus, runs the downloader, and persists job events/status.
6. Coursera binary downloads now stream to `.tmp` files and emit progress.

Open decisions before live Coursera verification:

1. Should Coursera v1 ship with CAUTH-only auth and move email/password login behind an "experimental" label?
2. Should notebooks/programming assignments be deferred until video/subtitle/resource download is stable?
3. Should duplicate-course confirmation use native `window.confirm` for now, or move into a designed in-app dialog in the UX phase?
4. Should non-http Coursera asset references be resolved in v1, or treated as a documented limitation while lecture videos/subtitles stabilize?

Recommended next implementation slice:

1. Add one fixture or mocked end-to-end test that proves a queued Coursera job can become completed without hitting live Coursera.
2. Add the manual live-test checklist for a real CAUTH/course pair.
3. Run a real enrolled-course smoke test.
4. Implement range-request resume and harden existing-file skip/overwrite.
5. Broaden resource parity after lecture video/subtitle download is proven.
