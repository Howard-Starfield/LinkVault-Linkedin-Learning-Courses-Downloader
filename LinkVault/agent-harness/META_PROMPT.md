# Fresh-Agent Meta Prompt

Use this when starting a new agent for the migration.

```text
You are working in:
C:\Users\howard\Downloads\Ai_script\Linkedin-Learning-Courses-Downloader-main

The user asked that migration-control work be kept inside:
C:\Users\howard\Downloads\Ai_script\Linkedin-Learning-Courses-Downloader-main\LinkVault

Read these first:
- LinkVault/agent-harness/README.md
- LinkVault/agent-harness/STATUS.md
- LinkVault/agent-harness/TODO.md
- LinkVault/agent-harness/DESKTOP_UAT.md
- LinkVault/agent-harness/RELEASE_HANDOFF.md
- LinkVault/agent-harness/REFERENCE_CONTRACT.md
- LinkVault/agent-harness/EDGE_CASE_MATRIX.md
- LinkVault/design.md

Visual references:
- LinkVault/reference.png
- Attached Image #1 in the original planning conversation if available

Product target:
Tauri 2 + Rust backend + React 19/Vite UI + Tailwind v4 design tokens + local Radix-style primitives + Lucide/Tabler icons + Sonner toasts + SQLite local cache.

Current scaffold:
`LinkVault/linkvault-tauri`

Current verification commands:
- `pnpm.cmd build` from `LinkVault/linkvault-tauri`
- `cargo test` from `LinkVault/linkvault-tauri/src-tauri`
- `pnpm.cmd run verify:visual` from `LinkVault/linkvault-tauri`
- `pnpm.cmd run verify:ui` from `LinkVault/linkvault-tauri`
- `pnpm.cmd run verify:tauri-smoke` from `LinkVault/linkvault-tauri`
- `pnpm.cmd run verify:release` from `LinkVault/linkvault-tauri`
- `pnpm.cmd tauri build --debug` from `LinkVault/linkvault-tauri`
- Optional manual visual check if needed: start Vite on another port, then capture screenshots with Playwright at `1536x1024`, `1280x800`, and `390x844`.

Current backend seams:
- `validate_li_at_token` Tauri command validates manual tokens through the Rust auth module and returns only session/header metadata.
- `validate_browser_token_source` reads browser cookie DB candidates and validates the first usable candidate without returning the token.
- Auth tests use fake LinkedIn home responses; do not rely on live LinkedIn checks before deterministic coverage exists.
- Browser cookie import copies SQLite DB/WAL/SHM files before reads, supports Firefox values, and supports Chrome/Edge encrypted Chromium values through Local State key unwrap plus `v10`/`v11` AES-GCM decrypt.
- Course metadata parsing has deterministic fixtures for detailed course metadata, selected-video details, transcript SRT generation, and expired-token/CSRF error handling.
- Selected-video fetch orchestration has fake-client tests for skip behavior, 1080 -> 720 fallback, no-downloadable-video, and expired-token propagation.
- Exercise URL refresh has deterministic fixtures for escaped direct URLs, Ambry URLs, filename matching, by-order fallback when counts align, integrated refresh during course fetch, and non-fatal refresh request failure.
- Safe exercise archive extraction has deterministic fixtures for valid zip extraction, non-zip skip behavior, unsafe path failure that keeps the zip and cleans temporary folders, duplicate wrapper folder collapse, unique destination naming, and delete-zip-only-after-success behavior.
- SQLite repository functions exist for settings, course cache, jobs, job events, and artifacts, with deterministic tests and a guard rejecting secret-like setting keys before SQLite writes.
- Cancellation-safe job lifecycle state transitions exist on top of the SQLite repository: queued -> active, queued -> cancelled, active -> completed/failed/cancelled, event recording, missing-job errors, and restart reconciliation that marks active jobs plus active/pending artifacts failed.
- Tauri startup now runs SQLite restart reconciliation, and Start Download now persists UI-safe settings, queued job rows, and `job.queued` events through `start_download_jobs` without accepting or storing plaintext token values.
- Bootstrap now restores UI-safe download preferences and recent persisted jobs into the queue UI, with browser-only Vite previews safely ignoring the Tauri command.
- A deterministic queued-job orchestration seam now exists in `src-tauri/src/download_orchestrator.rs`: it moves the oldest queued job to active, fetches course metadata through `CourseApiClient`, writes a UI-safe course cache payload without signed media/exercise URLs, creates pending video/subtitle/exercise artifact rows, appends metadata/artifact events, returns `Ok(None)` when no queued job exists, and marks jobs failed on metadata/planning failures.
- A deterministic artifact execution seam now exists in `src-tauri/src/artifact_downloader.rs`: URL-backed videos/exercises and inline subtitle text are written atomically through `.part` files, artifact status moves through active/completed/failed/cancelled in SQLite, exercise 404 fails only that artifact while continuing remaining downloads, cancellation before the next artifact cancels the job and remaining artifacts, and fatal non-exercise failures mark the job failed.
- `download_orchestrator::process_next_queued_job_and_download_artifacts` now connects queued-job activation, course metadata fetch/cache, artifact row planning, file execution, and job completion through fakeable clients.
- Live LinkedIn command boundaries now exist: `process_next_queued_download_with_li_at` validates a transient manual token and processes the next queued job, while `process_next_queued_download_from_browser_source` selects a browser token inside the backend and processes the next queued job without returning or persisting the raw token.
- `src-tauri/src/live_clients.rs` contains `AuthenticatedLinkedInClient`, a reqwest-backed client implementing both `CourseApiClient` and `ArtifactHttpClient` from a transient `li_at` token plus validated CSRF/session headers.
- Signed exercise URLs were removed from artifact 404 event messages and HTTP error display text.
- The Start Download UI now invokes the live queued-download command after queue persistence, refreshes persisted queue/activity state after processing, supports both manual token and browser token paths, and does not store raw tokens.
- Downloaded exercise zips are now auto-extracted inside the live artifact loop: valid zips are extracted and deleted only after successful extraction, warnings are recorded, and unsafe zip extraction failure marks only that exercise artifact failed while continuing remaining artifacts.
- Bootstrap now returns recent persisted job events, and the Activity/Completed UI surfaces render SQLite-backed events and terminal jobs instead of visible static completed/activity placeholders.
- Deterministic cancellation boundaries now cover metadata prefetch cancellation, post-response/pre-write artifact cancellation, pre-extraction zip cancellation, post-extraction zip completion with job cancellation, and before-next-artifact cancellation.
- A real UI cancel command path now exists: `cancel_active_download` sets shared atomic cancellation state, live processing passes that state into the orchestrator, and the Cancel button is enabled while processing and refreshes persisted queue/activity state after the request.
- Bootstrap now includes SQLite artifact-count summaries for each persisted recent job. Live Progress and Download Queue render persisted job/artifact counts instead of visible demo courses, and long course names/URLs/history labels have desktop and narrow render checks through browser-only `?preview=long-labels` state when Tauri IPC is unavailable.
- `pnpm.cmd run verify:visual` now runs a checked-in Playwright script that starts Vite, checks the reference desktop/laptop shell, verifies guarded/disabled/masked states, exercises `?preview=long-labels`, checks narrow scrolling/no horizontal overflow, and writes assertion screenshots under `output/playwright/`.
- `pnpm.cmd run verify:ui` now runs checked-in Playwright interaction assertions for invalid embedded/non-learning URLs, multiple LinkedIn Learning URLs with blank lines, visible Sonner success/error states, guarded Start Download behavior, preserved input order, course metadata shape-drift safe error handling, optional exercise 404 continuation, multiple-course queue ordering/progress, failed-course lifecycle behavior, repetitive artifact failure toast coalescing, keyboard navigation order, and no browser requests to LinkedIn domains.
- The React app has a browser-preview URL parser fallback that mirrors the Rust parser only when Tauri IPC is unavailable; live Tauri runtime still uses the Rust `parse_linkedin_course_urls` command.
- The React app has browser-preview command seams for token validation, queue persistence, and processing scenarios; these are inactive in real Tauri runtime and exist to exercise UI failure states without live LinkedIn calls.
- `EDGE_CASE_MATRIX.md` has been converted into initial UI test tickets in `TODO.md`.
- Failed-course lifecycle behavior is decided: processing handles one queued course at a time; if that course fails before artifact planning, it moves to terminal failed history while remaining courses stay queued in original order for a later run.
- Repetitive artifact failures now use one coalesced `Queued download processed with issues` warning toast rather than per-artifact failure toasts.
- Keyboard navigation coverage now verifies sidebar, setup form, actions, queue, and activity controls in logical order; the checkbox primitive gives focused download options meaningful accessible names.
- Local primitive coverage now includes Tooltip, Popover, Dialog, and guardedToast primitives. The settings icon opens a focus-returning dialog; the help icon opens an Escape-closeable popover; guarded folder picker behavior uses the shared toast helper.
- The Browse action now uses Tauri's native folder picker in desktop runtime through `@tauri-apps/plugin-dialog` / `tauri-plugin-dialog`, with `dialog:allow-open` granted in the default capability. Browser preview keeps a deterministic guarded fallback toast.
- `pnpm.cmd run verify:tauri-smoke` now checks native dialog plugin wiring, runs a debug Tauri build, launches `linkvault.exe`, waits through the startup/bootstrap smoke window, and terminates it cleanly without opening a blocking OS folder dialog.
- Desktop-only manual validation steps live in `LinkVault/agent-harness/DESKTOP_UAT.md`; release-prep gates and packaging decisions live in `LinkVault/agent-harness/RELEASE_HANDOFF.md`.
- The first packaging target is the release executable and the first installer target is NSIS. `pnpm.cmd run verify:release` builds with `--bundles nsis`, requires `src-tauri/target/release/linkvault.exe`, requires an NSIS `*-setup.exe`, smoke-launches the release executable, and terminates it cleanly.
- Next integration work should run the updated release verifier, then run the manual desktop UAT checklist on the Windows desktop and resolve installer branding/code-signing decisions.

Scope:
Build a LinkedIn Learning course downloader only. Preserve 1080p best-available default, fallback to lower resolutions, exercise file download, auto unzip, safe zip extraction, transcript/subtitle download, browser token import, manual token paste, cancellation, progress, and local SQLite cache.

Hard rules:
- Do not port Generic Video for MVP.
- Do not store plaintext LinkedIn tokens in SQLite.
- Do not ignore design.md; map UI decisions to its shell, primitive, accessibility, overlay, and governance rules.
- Do not ignore reference.png; use it as the first-screen screenshot target.
- Keep text inside controls/cards at desktop and narrow widths.
- Add deterministic tests for every backend edge case before relying on live LinkedIn checks.
- Update LinkVault/agent-harness/STATUS.md and TODO.md after each slice.

If code must be generated outside LinkVault/, record the exact new owned path in STATUS.md before editing.
```

## Self-Evolution Rules

Update this prompt when:

- The Tauri scaffold location is finalized.
- Test commands become concrete.
- A product-scope decision changes.
- A repeated implementation mistake appears.
- A new edge case is found in code or UAT.
