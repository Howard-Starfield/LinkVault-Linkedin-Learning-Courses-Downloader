# TODO

## Ready Before Scaffold

- [x] Confirm final Tauri scaffold location: `LinkVault/linkvault-tauri`.
- [x] Decide whether the Generic Video row should be hidden or disabled in the migrated sidebar: disabled visual context only.
- [x] Decide credential storage approach for `li_at`: do not store plaintext in SQLite; use a secret-store seam for live token persistence.
- [x] Convert `EDGE_CASE_MATRIX.md` into first Rust and UI test tickets. Initial concrete UI tickets are listed below.
- [x] Convert `REFERENCE_CONTRACT.md` into Playwright screenshot assertions once React exists. Covered by `pnpm.cmd run verify:visual`.

## Backend Parity Tasks

- [x] Port LinkedIn URL parser and course slug extraction.
- [x] Port token validation: trial prompt rejection, `JSESSIONID`, CSRF header behavior.
- [x] Port enterprise profile hash extraction and `x-li-identity` request header.
- [x] Complete Chrome/Edge encrypted cookie decryption for browser token import.
- [x] Port course metadata parsing.
- [x] Port selected-video fetch with 1080-first fallback.
- [x] Port exercise URL refresh, including escaped URLs and Ambry links.
- [x] Port safe zip extraction. Initial unsafe archive path guard exists.
- [x] Add SQLite repository functions for settings, course cache, jobs, job events, and artifacts.
- [x] Run SQLite restart reconciliation during Tauri setup.
- [x] Persist UI-safe settings, queued jobs, and queue events when Start Download is invoked.
- [x] Move queued jobs to active and cache UI-safe course metadata through a deterministic orchestration seam.
- [x] Create initial video/subtitle/exercise artifact rows from fetched course metadata.
- [x] Add deterministic artifact download execution with SQLite active/completed/failed/cancelled status updates.
- [x] Handle exercise artifact 404 by failing only that artifact and continuing remaining downloads.
- [x] Wire orchestration seam to live in-memory LinkedIn session and Tauri command boundary.
- [x] Wire Start Download UI to invoke live queued-download processing and refresh queue/activity state.
- [x] Auto-unzip downloaded exercise zips in the live artifact loop.
- [x] Load persisted queued/reconciled jobs into bootstrap queue UI state.
- [x] Load persisted completed/failed jobs into history UI state.
- [x] Add cancellation-safe download state transitions.

## UI Tasks

- [x] Build first screen from `REFERENCE_CONTRACT.md`.
- [x] Define Tailwind v4 tokens based on `design.md` and `reference.png`.
- [x] Build local primitives: button, icon button, textarea, input, select, checkbox, progress, tooltip, dialog/popover shell, toast adapter.
- [x] Replace visible demo progress streams with persisted active/completed/failed/cancelled job event state.
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
- [x] 1080 selected but only 720 is available.
- [x] Exercise file 404 while videos remain downloadable.
- [x] Zip with unsafe `../` path.
- [x] Zip with single duplicate root folder.
- [x] Cancel during metadata fetch, video download, and zip extraction. Deterministic cancellation boundaries now cover metadata prefetch, post-response/pre-write artifact transfer, pre-extraction zip handling, and post-extraction completion.
- [x] App restart with active/failed/completed jobs in SQLite.
