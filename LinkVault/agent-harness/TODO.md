# TODO

## Ready Before Scaffold

- [x] Confirm final Tauri scaffold location: `LinkVault/linkvault-tauri`.
- [x] Decide whether the Generic Video row should be hidden or disabled in the migrated sidebar: disabled visual context only.
- [x] Decide credential storage approach for `li_at`: do not store plaintext in SQLite; use a secret-store seam for live token persistence.
- [x] Decide first packaging target: release executable, with installers deferred until branding/signing are decided.
- [x] Decide first installer target: NSIS setup executable, with MSI deferred until branding/signing are decided.
- [x] Check in NSIS bundle config so plain `pnpm.cmd tauri build` emits the installer.
- [x] Convert `EDGE_CASE_MATRIX.md` into first Rust and UI test tickets. Initial concrete UI tickets are listed below.
- [x] Convert `REFERENCE_CONTRACT.md` into Playwright screenshot assertions once React exists. Covered by `pnpm.cmd run verify:visual`.

## Backend Parity Tasks

- [x] Port LinkedIn URL parser and course slug extraction.
- [x] Port token validation: trial prompt rejection, `JSESSIONID`, CSRF header behavior.
- [x] Port enterprise profile hash extraction and `x-li-identity` request header.
- [x] Complete Chrome/Edge encrypted cookie decryption for browser token import.
- [x] Port course metadata parsing.
- [x] Port selected-video fetch with 1080-first fallback.
- [x] Guard Best Available against LinkedIn `_1080` responses that visibly encode a sub-720 stream before falling back to 720.
- [x] Port exercise URL refresh, including escaped URLs and Ambry links.
- [x] Decode HTML-entity encoded Ambry query separators such as `&#61;`.
- [x] Skip empty Ambry placeholder URLs and keep non-empty Ambry links as exercise fallbacks.
- [x] Preserve existing direct named exercise ZIP URLs instead of replacing them with unmatched Ambry URLs by count.
- [x] Try alternate exercise artifact URLs when the first refreshed or metadata URL returns an HTTP failure.
- [x] Port safe zip extraction. Initial unsafe archive path guard exists.
- [x] Add SQLite repository functions for settings, course cache, jobs, job events, and artifacts.
- [x] Run SQLite restart reconciliation during Tauri setup.
- [x] Persist UI-safe settings, queued jobs, and queue events when Start Download is invoked.
- [x] Move queued jobs to active and cache UI-safe course metadata through a deterministic orchestration seam.
- [x] Create initial video/subtitle/exercise artifact rows from fetched course metadata.
- [x] Add deterministic artifact download execution with SQLite active/completed/failed/cancelled status updates.
- [x] Handle exercise artifact 404 by failing only that artifact and continuing remaining downloads.
- [x] Wire orchestration seam to live in-memory LinkedIn session and Tauri command boundary.
- [x] Carry LinkedIn home-response cookies transiently for live metadata/course-page requests without serializing or storing them.
- [x] Wire Start Download UI to invoke live queued-download processing and refresh queue/activity state.
- [x] Auto-unzip downloaded exercise zips in the live artifact loop.
- [x] Treat live exercise artifact download failures as non-fatal optional failures, not whole-course failures.
- [x] Fetch third-party exercise/CDN artifact URLs without LinkedIn session headers or cookies.
- [x] Fetch all file artifact URLs with legacy plain GET behavior, including LinkedIn Ambry exercise URLs.
- [x] Retry LinkedIn-host artifact URLs with authenticated session headers when the legacy plain request is non-success.
- [x] Treat successful `2xx` artifact responses such as `206 Partial Content` as downloadable files.
- [x] Add safe exercise artifact source diagnostics without signed query values for future HTTP 400 failures.
- [x] Record sanitized per-attempt HTTP statuses for failed exercise URL candidates.
- [x] Restore C#-style chapter folders and numbered video/subtitle file names.
- [x] Load persisted queued/reconciled jobs into bootstrap queue UI state.
- [x] Load persisted completed/failed jobs into history UI state.
- [x] Add cancellation-safe download state transitions.

## UI Tasks

- [x] Build first screen from `REFERENCE_CONTRACT.md`.
- [x] Define Tailwind v4 tokens based on `design.md` and `reference.png`.
- [x] Refine the first screen against `reference.png` with Linear-inspired dark density, exact reference copy, and a deterministic `?preview=reference` visual check.
- [x] Build local primitives: button, icon button, textarea, input, select, checkbox, progress, tooltip, dialog/popover shell, toast adapter.
- [x] Wire native Tauri folder picker for the download folder browse action, with browser-preview fallback coverage.
- [x] Add repeatable Tauri desktop smoke verification for dialog permission wiring and startup/bootstrap launch.
- [x] Add manual desktop UAT and release handoff checklists for the native runtime path.
- [x] Add repeatable release-build verification for the first packaging target.
- [x] Add repeatable NSIS installer verification for the first installer target.
- [x] Add non-installing installer artifact verification for filename, version, size, and PE header.
- [x] Assert checked-in Tauri bundle config in release verification.
- [x] Generate release handoff manifest with artifact SHA-256 hashes.
- [x] Replace visible demo progress streams with persisted active/completed/failed/cancelled job event state.
- [x] Poll SQLite-backed bootstrap state while a live queued-download command is running so the UI can update during long artifact downloads.
- [x] Add deterministic browser-preview coverage proving in-flight polling refreshes intermediate active artifact counts before completion.
- [x] Scope reference-preview-only fake progress/copy to preview jobs; real jobs compute progress from persisted completed, failed, and cancelled artifact counts.
- [x] Bound Recent Activity height and coalesce only long repeated artifact bursts so the activity panel does not keep growing forever.
- [x] Tighten app-shell typography and dark tokens toward a dense Linear-style desktop UI without decorative glow backgrounds.
- [x] Add repeatable Playwright visual assertions for desktop, laptop, narrow, long-label, disabled-scope, guarded-start, and masked-token checks.
- [x] Add repeatable Playwright interaction assertions for URL parse errors, blank-line multi-URL order, guarded start, visible toasts, and no live LinkedIn calls.
- [x] Validate desktop `1536x1024`, laptop `1280x800`, and narrow `390x844` layouts.

## UI Test Tickets From Edge Matrix

- [x] URL parser UI: embedded URL or non-learning URL shows a clear error/toast and does not persist jobs.
- [x] URL parser UI: multiple URLs with blank lines are accepted, blanks ignored, and order preserved in queued rows.
- [x] Course metadata UI: course JSON shape drift surfaces a safe visible error without raw unsafe response text.
- [x] Exercise 404 UI: failed optional exercise artifact appears as failed while remaining video/subtitle progress can continue.
- [x] Download lifecycle UI: multiple courses preserve queue order and per-course progress.
- [x] Download lifecycle UI: one failed course behavior is decided, documented, and represented in queue/history state.
- [x] Failure toast UI: repetitive artifact failures are coalesced or rate-limited so Sonner does not flood.
- [x] Keyboard navigation UI: sidebar, setup form, actions, queue, and activity controls are reachable in logical order.

## Edge Cases To Test First

- [x] Long course URL lines and multiple URLs. Long URL/history labels are covered by the `?preview=long-labels` visual check.
- [x] Long course/chapter/video titles. Long course/title rendering is covered by the `?preview=long-labels` desktop and narrow scroll screenshots.
- [x] Invalid token, expired token, no browser token found.
- [x] Browser cookie DB locked. Cookie DB/WAL/SHM copy-before-read behavior is covered by deterministic tests.
- [x] Browser cookie DB locked in one profile does not abort scanning other browser profiles.
- [x] 1080 selected but only 720 is available.
- [x] 1080 selected but LinkedIn returns a visibly lower stream URL.
- [x] Exercise file 404 while videos remain downloadable.
- [x] Zip with unsafe `../` path.
- [x] Zip with single duplicate root folder.
- [x] Cancel during metadata fetch, video download, and zip extraction. Deterministic cancellation boundaries now cover metadata prefetch, post-response/pre-write artifact transfer, pre-extraction zip handling, and post-extraction completion.
- [x] App restart with active/failed/completed jobs in SQLite.
