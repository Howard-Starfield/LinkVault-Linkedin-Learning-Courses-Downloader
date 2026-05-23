# Status

## 2026-05-23 NSIS Installer Verification Slice

Status: the release gate now produces and requires the first installer artifact.

Files changed:

- `LinkVault/linkvault-tauri/scripts/verify-release.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`
- `LinkVault/agent-harness/RELEASE_HANDOFF.md`

Implemented in this slice:

- Verified `pnpm.cmd tauri build --bundles nsis` works in the local Windows environment.
- Updated `pnpm.cmd run verify:release` to build with `--bundles nsis`.
- The release verifier now requires both `src-tauri/target/release/linkvault.exe` and an NSIS `*-setup.exe` under `src-tauri/target/release/bundle/nsis`.
- MSI remains deferred until installer branding and code-signing decisions are settled.

Validation evidence:

- `pnpm.cmd tauri build --bundles nsis` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd run verify:release` passed in `LinkVault/linkvault-tauri`.
- Release executable produced: `LinkVault/linkvault-tauri/src-tauri/target/release/linkvault.exe` at 15.61 MB.
- NSIS installer produced: `LinkVault/linkvault-tauri/src-tauri/target/release/bundle/nsis/LinkVault_0.1.0_x64-setup.exe` at 4.10 MB.
- Release executable stayed alive for the 5000ms startup smoke window before clean termination.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.

Current next slice:

Run the manual desktop UAT checklist on the Windows desktop and resolve installer branding/code-signing decisions.

## 2026-05-23 Release Verification Slice

Status: the first packaging target is decided as the release executable and has a repeatable verifier.

Files changed:

- `LinkVault/linkvault-tauri/scripts/verify-release.mjs`
- `LinkVault/linkvault-tauri/package.json`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`
- `LinkVault/agent-harness/RELEASE_HANDOFF.md`

Implemented in this slice:

- Added `pnpm.cmd run verify:release`.
- The release verifier runs `pnpm tauri build`, requires `src-tauri/target/release/linkvault.exe`, checks it is non-empty, lists any Tauri bundle artifacts emitted under `src-tauri/target/release/bundle`, launches the release executable through the startup smoke window, and terminates it cleanly.
- Decided the first shareable packaging target as the release executable; MSI/NSIS installer artifacts remain optional until icon, branding, and code-signing decisions are made.

Validation evidence:

- `pnpm.cmd run verify:release` passed in `LinkVault/linkvault-tauri`.
- Release executable produced: `LinkVault/linkvault-tauri/src-tauri/target/release/linkvault.exe` at 15.61 MB.
- Release executable stayed alive for the 5000ms startup smoke window before clean termination.
- No installer bundle artifacts were emitted under `LinkVault/linkvault-tauri/src-tauri/target/release/bundle` by the current Tauri config.
- `pnpm.cmd run verify:tauri-smoke` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.

Current next slice:

Run the manual desktop UAT checklist on the Windows desktop and resolve installer branding/code-signing decisions.

## 2026-05-23 Desktop UAT And Release Handoff Slice

Status: desktop-only validation and release-prep handoff steps are documented for the native runtime checks that should not be faked in browser preview.

Files changed:

- `LinkVault/agent-harness/DESKTOP_UAT.md`
- `LinkVault/agent-harness/RELEASE_HANDOFF.md`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added a Windows desktop UAT checklist covering startup, native folder picker, help/settings overlays, guarded download behavior, and evidence to record.
- Added a release handoff checklist covering the automated gate, manual pre-share gate, packaging command, and open release decisions.
- Kept OS-modal native folder picker validation as explicit manual UAT while retaining automated smoke coverage for Tauri plugin permission wiring and startup bootstrap.

Validation evidence:

- Documentation-only slice after the green Tauri smoke/UI/build/Rust/visual validation pass recorded below.

Current next slice:

Run the manual desktop UAT checklist on the Windows desktop, then decide the first packaging target: raw executable, release executable, MSI, or NSIS installer.

## 2026-05-23 Tauri Runtime Smoke Script Slice

Status: Tauri-only runtime surfaces now have a repeatable desktop smoke verifier.

Files changed:

- `LinkVault/linkvault-tauri/scripts/verify-tauri-smoke.mjs`
- `LinkVault/linkvault-tauri/package.json`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added `pnpm.cmd run verify:tauri-smoke`.
- The verifier asserts native dialog plugin wiring across npm, Cargo, Tauri builder registration, capability permissions, and the Browse action's directory picker call.
- The verifier runs `pnpm tauri build --debug`, asserts the debug executable exists, launches `linkvault.exe`, waits for the startup/bootstrap smoke window, and terminates the process cleanly.
- The desktop smoke avoids opening the native folder dialog directly so automation does not block on an OS modal picker.

Validation evidence:

- `pnpm.cmd run verify:tauri-smoke` passed in `LinkVault/linkvault-tauri`: static dialog wiring passed, `pnpm tauri build --debug` passed, and `linkvault.exe` stayed alive for the 5000ms startup smoke window before clean termination.
- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- The smoke build produced `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Run a manual desktop UAT pass against `linkvault.exe` for the native folder picker and overlay interactions, then prepare a release/package handoff checklist.

## 2026-05-23 Native Folder Picker Wiring Slice

Status: the download folder Browse action now uses the native Tauri folder picker in desktop runtime with deterministic browser-preview fallback coverage.

Files changed:

- `LinkVault/linkvault-tauri/package.json`
- `LinkVault/linkvault-tauri/pnpm-lock.yaml`
- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src-tauri/capabilities/default.json`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added `@tauri-apps/plugin-dialog` and `tauri-plugin-dialog`.
- Registered the dialog plugin in the Tauri builder and granted `dialog:allow-open` in the default capability.
- Replaced the Browse placeholder with `open({ directory: true, multiple: false })` in real Tauri runtime.
- Selected folders update the download folder field and show a success toast.
- Dialog errors show a safe failure toast without mutating the current folder value.
- Browser-only preview keeps a deterministic guarded fallback toast instead of attempting native dialog IPC.
- Extended `pnpm.cmd run verify:ui` to assert the preview fallback toast and unchanged folder value.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-folder-picker-preview.png`

Current next slice:

Run a real desktop smoke check for the Tauri runtime surfaces that browser preview cannot exercise directly: folder picker permission, settings dialog, and startup bootstrap.

## 2026-05-23 Local Primitive Completion Slice

Status: local primitive coverage now includes tooltip, dialog, popover, and guarded toast helper primitives with deterministic browser-preview coverage.

Files changed:

- `LinkVault/linkvault-tauri/src/components/primitives.tsx`
- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added local `Tooltip`, `Popover`, `Dialog`, and `guardedToast` primitives matching the existing compact LinkVault shell style.
- Wired the header settings icon to a focus-returning settings dialog.
- Wired the sidebar help icon to an Escape-closeable popover.
- Moved the guarded folder picker placeholder onto the shared guarded toast helper.
- Extended `pnpm.cmd run verify:ui` to assert tooltip visibility, popover Escape close, dialog focus on open, dialog Escape close, and focus return to the trigger.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-primitive-overlays.png`

Current next slice:

Run a final commit checkpoint for the accumulated LinkVault UI coverage and primitive work, keeping unrelated parent repo changes unstaged.

## 2026-05-23 Keyboard Navigation UI Interaction Test Slice

Status: core keyboard navigation now has deterministic browser-preview UI coverage across sidebar, setup form, actions, queue, and activity controls.

Files changed:

- `LinkVault/linkvault-tauri/src/components/primitives.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added default `aria-label` propagation for the local checkbox primitive so focused download-option checkboxes expose meaningful names.
- Added browser-preview keyboard traversal assertions for the sidebar nav, settings action, setup form fields, download option checkboxes, token import action, queue action, and activity actions.
- The verifier asserts Start Download remains guarded during keyboard traversal.
- The assertion screenshot was refreshed under `output/playwright/`.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-keyboard-navigation.png`

Current next slice:

Finish the local primitive coverage gap: add or harden the remaining tooltip, dialog/popover shell, and toast adapter primitives as needed by the LinkVault UI.

## 2026-05-23 Repetitive Artifact Failure Toast UI Slice

Status: repeated artifact failures now surface as one coalesced processing warning toast with deterministic browser-preview coverage.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added `showProcessedDownloadToast` so processed jobs with failed or cancelled artifacts show `Queued download processed with issues` instead of a success toast.
- Kept successful processed jobs on the existing success toast path.
- Added a browser-preview `repetitive-artifact-failures` processing scenario with 6 failed exercise artifacts coalesced into one safe activity event.
- Extended `pnpm.cmd run verify:ui` to assert exactly one Sonner failure-related toast is visible for repeated failures.
- The UI assertion proves signed artifact URLs and manual token values are not rendered.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-repetitive-artifact-failures.png`

Current next slice:

Add deterministic UI coverage for keyboard navigation: sidebar, setup form, actions, queue, and activity controls should be reachable in a logical order.

## 2026-05-23 Failed-Course Lifecycle UI Interaction Test Slice

Status: one failed course behavior is decided, documented, and covered by deterministic browser-preview UI assertions.

Behavior decision:

- Processing is one queued course at a time.
- If the processed course fails before artifact planning, that course moves to terminal failed history.
- Remaining queued courses stay queued in their original order for a later processing run.
- Safe UI state must not invent artifact progress for the failed course or expose unsafe response/token values.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added a browser-preview `failed-course-lifecycle` processing scenario.
- The first queued course becomes terminal failed with no artifact plan.
- The second queued course remains visible in the active queue with its own artifact counts.
- Extended `pnpm.cmd run verify:ui` to assert queue summary `1 queued - 1 failed`, terminal failed history, preserved remaining queued course state, no invented artifact progress, and safe activity messaging.
- The UI assertion proves unsafe backend response bodies, secret-like backend values, and manual token values are not rendered.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-failed-course-lifecycle.png`

Current next slice:

Add deterministic UI coverage for repetitive artifact failure toast behavior: coalesce or rate-limit repeated failures so Sonner does not flood.

## 2026-05-23 Multi-Course Progress UI Interaction Test Slice

Status: multiple-course lifecycle ordering now has deterministic browser-preview UI coverage with per-course artifact progress and no live LinkedIn calls.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added a browser-preview `multi-course-progress` processing scenario that leaves the first queued course active and the second course queued.
- The first course now exposes partial artifact counts in preview state: 3 of 6 complete, including per-type video/subtitle/exercise counts.
- The second course exposes its own queued artifact plan instead of inheriting the first course's progress.
- Extended `pnpm.cmd run verify:ui` to assert visible course order, queue summary `1 active - 1 queued`, per-course artifact summaries, per-type progress rows, and safe activity events.
- The UI assertion proves internal queue-only values and manual token values are not rendered.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-multi-course-progress.png`

Current next slice:

Decide and document one failed course behavior in the download lifecycle UI, then represent it in deterministic queue/history state without live LinkedIn calls.

## 2026-05-23 Exercise 404 UI Interaction Test Slice

Status: optional exercise 404 handling now has deterministic browser-preview UI coverage showing failed exercise state while video/subtitle work continues.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added a browser-preview `exercise-404` processing scenario that marks the queued preview job completed with 2 completed artifacts and 1 failed optional exercise artifact.
- The preview scenario records safe activity events for the failed exercise, continued video completion, and continued subtitle completion.
- Updated artifact summary text to show specific failed/cancelled artifact counts instead of a generic blocked count.
- Extended `pnpm.cmd run verify:ui` to assert that the exercise 404 flow leaves no active queue row, shows terminal history, exposes the failed optional exercise count, and proves video/subtitle work continued.
- The UI assertion proves signed exercise URLs and manual token values are not rendered.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-exercise-404.png`

Current next slice:

Implement deterministic UI coverage for download lifecycle ordering: multiple courses should preserve queue order and expose per-course progress without live LinkedIn calls.

## 2026-05-23 Course Shape-Drift UI Interaction Test Slice

Status: course metadata shape-drift handling now has deterministic browser-preview UI coverage with safe error text and no raw response/token leakage.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added browser-preview command seams for manual token validation, queue persistence, and processing while keeping real Tauri runtime on the existing Rust commands.
- Added a `metadata-shape-drift` preview scenario that queues a safe local job, simulates the backend course metadata shape error, moves the preview job to failed, and records a safe activity event.
- Extended `pnpm.cmd run verify:ui` to assert the shape-drift flow: Start Download stays guarded before token/session, valid URL plus token enables processing, the visible toast says `Download processing failed`, and the safe error text is `LinkedIn course metadata shape changed`.
- The UI assertion proves the raw unsafe metadata body and manual token value are not rendered, and that the failed job appears as terminal history with no active queue row.
- Refreshed the UI assertion screenshot for this flow under `output/playwright/`.

Validation evidence:

- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshot was refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-course-shape-drift.png`

Current next slice:

Implement deterministic UI coverage for the optional exercise 404 path: browser-preview processing should show one failed exercise artifact while remaining video/subtitle work can continue, without exposing signed exercise URLs.

## 2026-05-23 URL Parser UI Interaction Test Slice

Status: URL parser edge cases now have repeatable browser-preview interaction coverage without live LinkedIn calls.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/scripts/verify-ui.mjs`
- `LinkVault/linkvault-tauri/package.json`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added `pnpm.cmd run verify:ui`, backed by `scripts/verify-ui.mjs`.
- The UI verifier starts Vite on the first free local port from `1430`, opens Chromium, and asserts interactive URL parsing behavior in browser preview.
- Invalid embedded/non-learning URLs now surface a visible `Invalid course URL` Sonner error, keep Start Download guarded, keep the persisted queue empty, and do not contact LinkedIn domains.
- Multiple LinkedIn Learning URLs with blank lines now validate in order, ignore blanks, normalize to `https://www.linkedin.com/learning/{slug}`, show a visible success toast, and render a pre-queue validated-course preview.
- Added a browser-preview URL parser fallback that mirrors the Rust parser when Tauri IPC is unavailable; live Tauri runtime still uses the Rust `parse_linkedin_course_urls` command.
- Course URL textarea edits now clear stale validated previews so the visible queue state matches the current input.

Validation evidence:

- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd run verify:ui` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Interactive assertion screenshots were refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-invalid-url.png`
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-ui-multiple-urls.png`

Current next slice:

Implement deterministic UI coverage for safe backend processing failures, starting with the course JSON shape-drift path: browser-preview command mocks should surface a safe visible error without raw unsafe response text, while preserving guarded Start Download and persisted queue/activity invariants.

## 2026-05-23 Repeatable Visual Assertion Script Slice

Status: reference and long-label visual checks are now repeatable through a checked-in Playwright assertion script.

Files changed:

- `LinkVault/linkvault-tauri/scripts/verify-visual.mjs`
- `LinkVault/linkvault-tauri/package.json`
- `LinkVault/linkvault-tauri/pnpm-lock.yaml`
- `LinkVault/agent-harness/STATUS.md`
- `LinkVault/agent-harness/TODO.md`
- `LinkVault/agent-harness/META_PROMPT.md`

Implemented in this slice:

- Added `pnpm.cmd run verify:visual`, backed by `scripts/verify-visual.mjs`.
- Added `playwright` as a dev dependency so visual assertions run from the repo without transient `npx` argument handling.
- The visual verifier starts Vite on the first free local port from `1422`, opens Chromium, and asserts the reference layout at `1536x1024` and `1280x800`.
- The verifier checks MVP scope and security UI invariants: Generic Video remains disabled, Start Download is guarded before required input, and the token input stays password-masked.
- The verifier exercises `?preview=long-labels` at desktop and `390x844`, scrolls the mobile shell to queue/activity sections, and fails on horizontal overflow or clipped button text.
- The verifier writes assertion screenshots under `LinkVault/linkvault-tauri/output/playwright/`.
- Converted the first remaining UI rows from `EDGE_CASE_MATRIX.md` into concrete TODO test tickets.

Validation evidence:

- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd run verify:visual` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Assertion screenshots were refreshed:
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-visual-assert-desktop.png`
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-visual-assert-laptop.png`
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-visual-assert-long-desktop.png`
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-visual-assert-long-mobile-queue.png`
  - `LinkVault/linkvault-tauri/output/playwright/linkvault-visual-assert-long-mobile-activity.png`

Current next slice:

Implement the first interactive UI test ticket from `TODO.md`: deterministic browser-preview coverage for invalid/non-learning URLs and multiple URLs with blank lines, including Start Download guarding and visible toast/error behavior without live LinkedIn calls.

## 2026-05-23 Persisted Artifact Progress UI Slice

Status: remaining visible live-progress and queue-progress demo placeholders are replaced with SQLite-backed job/artifact counts.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/src/index.css`

Implemented in this slice:

- Bootstrap now summarizes persisted artifact rows for each recent job, including total/completed/failed/cancelled/active/pending/skipped counts and video/subtitle/exercise completed totals.
- Added deterministic backend coverage proving bootstrap returns persisted artifact counts beside a saved job without exposing token or signed URL data.
- Live Progress now renders the active/queued persisted job summary from SQLite artifact counts, or an empty persisted state when no live job exists.
- Download Queue no longer falls back to hardcoded demo courses; active/queued rows now render persisted artifact progress and per-type counts.
- History rows now include persisted artifact completion counts and keep long output paths truncated with the full path in the title.
- Added a browser-only `?preview=long-labels` render seed used only when Tauri IPC is unavailable, for deterministic long label/URL visual checks.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-artifact-counts-desktop.png`.
- Long-label desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-long-labels-desktop.png`.
- Long-label narrow scroll checks captured at `390x844`: `LinkVault/linkvault-tauri/output/playwright/linkvault-long-labels-mobile-scrolled.png` and `LinkVault/linkvault-tauri/output/playwright/linkvault-long-labels-mobile-activity.png`.

Current next slice:

Convert the reference and long-label visual checks into a repeatable Playwright assertion script, then start turning the remaining `EDGE_CASE_MATRIX.md` UI rows into concrete test tickets.

## 2026-05-23 Cancel Command UI Wiring Slice

Status: the app now has a real Tauri cancellation request path and the Cancel button requests cancellation while processing.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`

Implemented in this slice:

- Added shared `DownloadCancellation` state to `LinkVaultState` using an atomic cancellation flag.
- Registered `cancel_active_download` Tauri command.
- Live queued-download commands reset cancellation at process start and pass the shared cancellation flag into the orchestrator.
- Added deterministic state coverage proving cancellation requests are recorded and reset for a new processing run.
- The Cancel button is now enabled only while processing, invokes `cancel_active_download`, shows a Sonner cancellation-request toast, and refreshes persisted queue/activity state.
- Cancel button text changes to `Cancelling` while the request command is in flight.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 78 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-cancel-ui-desktop.png`.

Current next slice:

Replace the remaining demo live-progress and queue-progress placeholder visuals with persisted artifact/job counts from SQLite, then add focused render checks for long course names and long URL/history labels.

## 2026-05-23 Cancellation Boundary Slice

Status: cancellation handling now covers metadata-start, post-response/pre-write artifact transfer, and exercise zip extraction boundaries with deterministic tests.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/artifact_downloader.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/download_orchestrator.rs`

Implemented in this slice:

- `process_next_queued_job_and_download_artifacts` now checks cancellation immediately after a queued job becomes active and before metadata fetch begins, then transitions the job to `cancelled` without calling LinkedIn metadata clients.
- Artifact downloads now check cancellation after a URL response returns but before the file is written, preventing a cancelled transfer from producing a completed artifact.
- Exercise zip downloads now check cancellation after the zip is written but before extraction, keeping the downloaded zip, marking the artifact cancelled with its downloaded size, and avoiding extraction side effects.
- Exercise zip extraction now checks cancellation after extraction completes; the current artifact remains completed, extracted output is preserved, and the job transitions to cancelled before remaining artifacts.
- Remaining pending artifacts are marked `cancelled` with job events when cancellation stops the loop.
- Deterministic tests cover metadata prefetch cancellation, post-response/pre-write video cancellation, pre-extraction zip cancellation, post-extraction zip cancellation, and existing before-next-artifact cancellation.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 77 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- No screenshot was required for this backend-only slice.

Current next slice:

Wire a real UI cancel command path: add a backend cancellation request/state seam, enable the Cancel button while a job is processing, and refresh persisted job/activity state after cancellation.

## 2026-05-23 Persisted History And Activity UI Slice

Status: bootstrap now returns recent persisted job events, and the Activity/Completed surfaces render persisted SQLite state instead of visible static completed/activity placeholders.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`

Implemented in this slice:

- Added `recent_events` to `bootstrap_state`, built from recent persisted jobs and sorted newest-first across job events.
- Added a serializable persisted job-event response shape without exposing token or signed URL data.
- Extended bootstrap deterministic coverage to prove failed/completed job events are returned with persisted jobs.
- Recent Activity now renders persisted job events with status-colored timeline markers and an empty persisted state when no events exist.
- Completed now renders terminal SQLite jobs (`completed`, `failed`, `cancelled`) with status icons, output/source details, and an empty persisted state when no terminal jobs exist.
- Download Queue now separates active/queued jobs from terminal history while keeping the compact shell layout.

Validation evidence:

- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 73 tests passed.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-history-ui-desktop.png`.

Current next slice:

Add deterministic cancellation checks at metadata, in-flight artifact download boundaries, and zip extraction boundaries so jobs cannot remain active or leave misleading artifact state when cancellation is requested mid-work.

## 2026-05-23 Exercise Zip Extraction Loop Slice

Status: downloaded exercise zips are now auto-extracted inside the live artifact execution loop, with failed extraction isolated to the exercise artifact.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/artifact_downloader.rs`

Implemented in this slice:

- Integrated the existing safe exercise archive extractor after successful `exercise_zip` downloads.
- Valid exercise zips now extract to the course folder, collapse duplicate wrapper folders through the archive seam, delete the zip only after successful extraction, and record `artifact.extracted` before `artifact.completed`.
- Exercise zip extraction warnings, such as delete-after-extract failure, are recorded as `artifact.extraction.warning` while keeping the artifact completed.
- Unsafe or invalid exercise zips now mark only that exercise artifact failed, keep the downloaded zip, append a safe `artifact.failed` event, and continue remaining artifacts.
- Failed exercise zip extraction preserves the downloaded file size on the failed artifact row for local diagnostics.
- Deterministic tests cover valid zip extraction/delete/event recording and unsafe zip failure/zip retention/remaining-video continuation.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 73 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- No screenshot was required for this backend-only slice.

Current next slice:

Load persisted completed/failed jobs and recent job events into the History/Completed UI surfaces, replacing the remaining static completed/activity placeholders with SQLite-backed state.

## 2026-05-23 Live Download UI Wiring Slice

Status: Start Download now persists queued jobs, invokes the live queued-download backend command, and refreshes persisted queue/activity state after processing.

Files changed:

- `LinkVault/linkvault-tauri/src/App.tsx`

Implemented in this slice:

- Refactored bootstrap loading into a reusable refresh path so the UI can reload persisted preferences/jobs after live processing.
- Start Download now calls `start_download_jobs`, then processes the next queued job with `process_next_queued_download_with_li_at` for manual tokens or `process_next_queued_download_from_browser_source` for imported browser sessions.
- Live processing uses transient token/session inputs only; raw tokens are not written into SQLite.
- Processing state disables token import/start actions and updates button text while work is running.
- Browser source changes now clear the previously validated imported session so stale browser-session metadata is not reused.
- Queue header/footer now summarize persisted job statuses dynamically.
- Recent Activity now prepends a processed/no-work summary from the live queued-download response.

Validation evidence:

- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 71 tests passed.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-live-ui-desktop.png`.

Current next slice:

Add exercise zip extraction after successful zip downloads in the live artifact loop, with deterministic tests proving safe extraction, delete-zip-only-after-success behavior, and artifact/job event updates.

## 2026-05-23 Live Session Command Boundary Slice

Status: live LinkedIn session clients and Tauri command boundaries now exist for processing the next queued download without storing plaintext tokens in SQLite.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/artifact_downloader.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/live_clients.rs`

Implemented in this slice:

- Added `live_clients` backend module.
- Added `AuthenticatedLinkedInClient`, a reqwest-backed client that implements both `CourseApiClient` and `ArtifactHttpClient`.
- Live client construction uses a transient `li_at` token plus validated CSRF/session headers; it does not write token values into SQLite.
- Registered `process_next_queued_download_with_li_at` Tauri command for manual-token execution.
- Registered `process_next_queued_download_from_browser_source` Tauri command for browser-token execution while keeping the selected browser token inside the backend call.
- Added command helper response shape for processed/no-work plus completed/failed/cancelled artifact counts.
- Removed signed exercise URLs from artifact 404 event messages and HTTP error display text.
- Deterministic tests cover live-client cookie/header construction, empty-token rejection, no-work command mapping, and exercise 404 URL redaction.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 71 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- No screenshot was required for this backend-only slice.

Current next slice:

Wire the Start Download UI to invoke the live queued-download command after queue persistence, refresh persisted queue/activity state after processing, and then add exercise zip extraction after successful zip downloads in the artifact loop.

## 2026-05-23 Artifact Download Execution Slice

Status: queued-job orchestration can now continue into deterministic artifact file execution and SQLite progress/status updates.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/artifact_downloader.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/download_orchestrator.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`

Implemented in this slice:

- Added `artifact_downloader` backend module.
- Added a fakeable `ArtifactHttpClient` seam for downloading URL-backed artifacts without live network tests.
- Added `ArtifactDownloadSource` support for URL-backed video/exercise files and inline subtitle text.
- Artifact writes are atomic through `.part` files and directory creation before rename.
- Artifact execution records `active`, `completed`, `failed`, and `cancelled` statuses in SQLite.
- Exercise file or zip HTTP 404 marks only that exercise artifact failed and continues remaining artifacts.
- Fatal non-exercise download failures mark the active job failed.
- Cancellation before the next artifact marks the job cancelled and records remaining artifacts as cancelled.
- Successful artifact execution transitions the active job to completed, allowing a completed job to retain failed optional exercise artifacts.
- `download_orchestrator` now exposes `process_next_queued_job_and_download_artifacts`, connecting queue activation, metadata planning, artifact row creation, file execution, and job completion in one deterministic backend path.
- Deterministic tests cover URL downloads, subtitle writes, exercise 404 continuation, cancellation, and orchestrated queue -> metadata -> artifact execution.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 68 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- No screenshot was required for this backend-only slice.

Current next slice:

Wire the deterministic backend execution path to live in-memory LinkedIn session clients and the Tauri command boundary: build reqwest-backed course/artifact clients using validated session headers without storing plaintext tokens, start queued downloads after validation, surface progress to the UI, and add exercise zip extraction after successful zip downloads.

## 2026-05-23 Queued Job Orchestration Slice

Status: queued SQLite jobs can now be advanced into active metadata/artifact planning through a deterministic backend orchestration seam.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/download_orchestrator.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`

Implemented in this slice:

- Added `download_orchestrator` backend module.
- `process_next_queued_job` loads the oldest queued job, transitions it to active, fetches course metadata through the existing `CourseApiClient` seam, and keeps no-work behavior as `Ok(None)`.
- Course metadata is written to `course_cache` using a UI-safe payload that includes course structure and availability flags but does not store signed media/exercise URLs.
- Initial pending artifact rows are created for selected videos, subtitles with transcripts, and exercise files/zips.
- Orchestration appends `course.metadata.cached` and `artifacts.planned` events after successful planning.
- Metadata/fetch/planning failures transition the active job to failed and append `job.failed`, avoiding orphan active jobs.
- Deterministic in-memory SQLite tests cover successful planning, no queued job, and metadata failure.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 64 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- No screenshot was required for this backend-only slice.

Current next slice:

Wire the orchestration seam to a live in-memory LinkedIn session and the file download loop: run queued jobs after validated auth, download planned video/subtitle/exercise artifacts without persisting plaintext tokens, handle exercise 404 as artifact failure while continuing remaining work, and add cancellation checks during metadata/download/archive boundaries.

## 2026-05-23 Persisted Queue Bootstrap Slice

Status: persisted download preferences and recent SQLite jobs now load through bootstrap and into the queue UI.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/cache.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`

Implemented in this slice:

- Added recent-job read support in the SQLite repository, ordered by latest update.
- `bootstrap_state` now reads from managed SQLite state instead of returning only static defaults.
- Bootstrap response now includes saved UI-safe download preferences and recent persisted jobs.
- Saved preferences restore output folder, selected quality, delay, browser source, and download toggles on app load.
- Recent persisted jobs hydrate the queue UI when available.
- Browser-only Vite previews gracefully ignore the Tauri-only bootstrap command.
- Deterministic tests cover recent-job ordering and bootstrap loading of saved preferences plus persisted/reconciled job state.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 61 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-bootstrap-desktop.png`.

Current next slice:

Start the live downloader orchestration loop for queued jobs: move queued jobs to active, fetch/cache course metadata, create initial video/subtitle/exercise artifact rows, and keep failures/cancellation represented through SQLite events.

## 2026-05-23 Startup And Queue Persistence Wiring Slice

Status: SQLite repository wired into Tauri startup and the first Start Download queue seam.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`

Implemented in this slice:

- Tauri setup now opens the SQLite cache, initializes schema, runs restart reconciliation for active jobs, and manages the database path for commands.
- Added `start_download_jobs` Tauri command.
- `start_download_jobs` reparses LinkedIn Learning course URLs on the backend before persistence.
- The command persists UI-safe download preferences to SQLite without accepting or storing plaintext `li_at`, token, cookie, or authorization values.
- The command inserts queued job rows for each parsed course URL and appends `job.queued` events with safe source URL/delay metadata.
- The existing Start Download UI path now validates the token/session as before, then calls `start_download_jobs` and reports persisted queued jobs.
- Queue header now reflects persisted queued job count after the command returns.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 59 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Local Vite render check passed on `http://127.0.0.1:1422`.
- Desktop screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-wiring-desktop.png`.

Current next slice:

Wire persisted queue state back into bootstrap/history surfaces and continue live downloader orchestration: read queued/reconciled jobs from SQLite on app load, display persisted queue state, and start moving queued jobs through metadata/course-cache/artifact writes.

## 2026-05-23 Cancellation-Safe Job Lifecycle Slice

Status: cancellation-safe job status transitions and restart reconciliation added to the SQLite repository.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/cache.rs`

Implemented in this slice:

- Validated job lifecycle transition API on top of SQLite jobs.
- Allowed transitions are now constrained to `queued -> active`, `queued -> cancelled`, and `active -> completed/failed/cancelled`.
- Terminal jobs cannot be moved back to active/completed/failed/cancelled through the transition API.
- Missing jobs return an explicit `JobNotFound` error.
- Lifecycle transitions can append deterministic job events such as `job.active`, `job.cancelled`, and `job.failed`.
- Restart reconciliation finds jobs left in `active`, marks them `failed`, appends a recovery event, and marks active/pending artifacts for those jobs as `failed`.
- Deterministic tests prove active jobs do not remain active after app restart and terminal/queued jobs are left alone.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 57 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Wire the SQLite repository into app startup and the first downloader orchestration seam: run restart reconciliation during Tauri setup, persist UI-safe settings, and begin recording queued job rows/events when Start Download is invoked.

## 2026-05-23 SQLite Repository Slice

Status: SQLite persistence repository functions added behind the existing cache schema.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/cache.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`

Implemented in this slice:

- `cache` is now a public backend module for future command/download orchestration wiring.
- Settings JSON upsert/read operations with deterministic timestamps.
- Secret-like setting keys such as `li_at`, `token`, `cookie`, and `authorization` are rejected before SQLite writes.
- Course cache upsert/read operations keyed by course slug.
- Job insert/read/list-by-status/update-status operations.
- Job event append/list operations with SQLite foreign-key cascade coverage.
- Artifact upsert/list/update-status operations with SQLite foreign-key cascade coverage.
- Deterministic repository tests cover settings, course cache, jobs, job events, artifacts, cascade behavior, and the no-plaintext-secret schema invariant.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 54 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Add cancellation-safe job lifecycle state transitions on top of the SQLite repository: queued -> active -> completed/failed/cancelled, restart reconciliation for active jobs, and deterministic tests proving active jobs do not remain active forever after app restart.

## 2026-05-23 Safe Exercise Archive Extraction Slice

Status: safe exercise zip extraction ported as a Rust backend seam.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/exercise_archive.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`

Implemented in this slice:

- `extract_zip_and_delete_archive` backend seam matching the C# archive extractor result shape.
- Non-zip exercise files are skipped and kept in place.
- Missing archive paths fail without attempting extraction.
- Zip extraction happens inside a temporary `.extracting-*` directory.
- Archive member paths are checked through the existing safe relative archive path guard before writing.
- Unsafe archive paths fail extraction, keep the zip, and clean temporary extraction folders.
- Valid zips extract into a destination folder named from the archive stem and delete the zip only after successful extraction.
- A single top-level folder matching the archive stem is collapsed to avoid duplicate wrapper folders.
- Existing destination folders are preserved by creating `name (2)`, `name (3)`, etc.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 49 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Begin SQLite-backed persistence beyond schema: settings/course-cache/job/job-event/artifact repository functions with deterministic tests, while continuing to keep plaintext LinkedIn tokens out of SQLite.

## 2026-05-23 Exercise URL Refresh Slice

Status: exercise URL refresh ported and integrated into the course fetch flow.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/course.rs`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`

Implemented in this slice:

- LinkedIn course page URL builder for `https://www.linkedin.com/learning/{courseSlug}`.
- Course fetch flow now attempts exercise URL refresh after detailed metadata parsing.
- Exercise refresh failure is non-fatal, preserving the stale metadata URL and continuing selected-video work.
- Escaped direct exercise file URL extraction, including LinkedIn-style escaped slashes and query parameters.
- Escaped Ambry exercise URL extraction.
- Case-insensitive exercise URL de-duplication.
- Filename-based refreshed URL matching.
- By-order unmatched URL assignment when unmatched file count and unmatched URL count align.
- Deterministic fake-client coverage for integrated refresh, selected-video continuation, and non-fatal refresh failure.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 44 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Port safe exercise archive extraction: valid zip extraction, non-zip skip behavior, unsafe path failure that keeps the zip, duplicate wrapper folder collapse, and delete-zip-only-after-success behavior.

## 2026-05-23 Selected Video Fetch Orchestration Slice

Status: selected-video fetch orchestration added with fake-client coverage.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/course.rs`

Implemented in this slice:

- `CourseApiClient` trait for deterministic course API orchestration tests.
- Course fetch flow that requests detailed course metadata first.
- Selected-video detail requests are skipped when both video and subtitle downloads are disabled.
- Selected-video detail flow applies the configured quality fallback order.
- 1080 -> 720 fallback is covered with fake API responses where 1080 has no progressive URL.
- No-downloadable-video error is surfaced when no fallback resolution has a download URL.
- Expired-token/CSRF metadata response propagates as a safe expired-token error.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 37 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Port exercise URL refresh with deterministic HTML fixtures: decode direct escaped exercise URLs, decode Ambry URLs, match refreshed URLs to exercise filenames, and assign unmatched URLs by order when counts align.

## 2026-05-23 Course Metadata Parser Slice

Status: deterministic course metadata parsing seam added.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/course.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`

Implemented in this slice:

- Rust course metadata model for course slug, title, chapters, videos, and exercise files.
- LinkedIn detailed course metadata URL builder.
- LinkedIn selected-video URL builder.
- Parser for `elements[0].title`, `elements[0].chapters`, and `elements[0].exerciseFiles`.
- Parser for `elements[0].selectedVideo` title, duration, progressive URL, and transcript lines.
- SRT transcript formatting matching the C# behavior: line end time is the next line start, or final video duration.
- Selected-video skip decision when both video and subtitle downloads are off.
- CSRF response handling maps to an expired-token error without including raw response text.
- Invalid JSON/shape errors avoid raw response dumps.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 33 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Current next slice:

Implement selected-video fetch orchestration with deterministic fake-client tests: fetch course metadata, optionally skip selected-video detail calls when videos/subtitles are off, apply 1080 -> 720 -> 540 -> 360 fallback using parsed selected-video responses, and surface expired-token/course-shape errors safely.

## 2026-05-23 Chromium Encrypted Cookie Decryption Slice

Status: Chrome/Edge encrypted cookie decryption implemented behind the browser-cookie seam.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/browser_cookies.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`

Implemented in this slice:

- Windows DPAPI unprotect boundary for Chromium encrypted values via `CryptUnprotectData`.
- Local State `os_crypt.encrypted_key` parsing and DPAPI-prefix stripping.
- AES-256-GCM decrypt support for Chromium `v10` and `v11` cookie payloads.
- Live Chrome/Edge browser import now builds a `ChromiumCookieDecoder` from each browser's `Local State` path before reading copied cookie DBs.
- Legacy Chromium encrypted cookie fallback attempts DPAPI unprotect directly.
- Deterministic tests for Local State key unwrapping through an injected protector.
- Deterministic tests for `v10` and `v11` AES-GCM cookie payload decrypt and wrong-key rejection.

Validation evidence:

- `cargo fmt` passed in `LinkVault/linkvault-tauri/src-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 26 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Security notes:

- Encrypted browser cookie values are decrypted in memory only for validation candidate selection.
- The validation command still returns only session/header metadata and does not return or persist plaintext `li_at`.
- SQLite cache schema still contains no plaintext token/cookie columns.

Current next slice:

Port course metadata parsing with deterministic JSON fixtures: detailed course title/chapters/videos/exercise files, CSRF-expired response handling without raw unsafe dumps, and skip selected-video calls when videos/subtitles are disabled.

## 2026-05-23 Browser Cookie Import Slice

Status: browser cookie import foundation added.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/browser_cookies.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/auth.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src/App.tsx`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`

Implemented in this slice:

- Browser source model for Chrome, Edge, and Firefox.
- Chrome/Edge profile cookie DB discovery under `LOCALAPPDATA`.
- Firefox profile cookie DB discovery under `APPDATA`.
- SQLite cookie DB copy-before-read behavior, including `-wal` and `-shm` sidecar copy.
- Chromium cookie table reader for `li_at` values across `.www.linkedin.com`, `www.linkedin.com`, and `.linkedin.com`.
- Firefox `moz_cookies` reader for `.www.linkedin.com` `li_at` values.
- Distinct, non-empty token candidate extraction.
- Adapter seam for Chromium encrypted cookie values.
- Tauri command `validate_browser_token_source` that reads browser candidates, validates the first usable one, and returns only session/header metadata.
- UI Import Token action now calls `validate_browser_token_source`; manual Start Download validates pasted `li_at` through `validate_li_at_token` before queueing.

Validation evidence:

- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 22 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Known gap:

- Chrome and Edge modern encrypted cookie values are behind the `ChromiumCookieValueDecoder` adapter and are not yet decrypted with Windows DPAPI/AES-GCM. Plaintext Chromium values and Firefox values are covered; the next slice should implement and test Windows Chromium decryption.

Current next slice:

Implement Windows Chromium cookie decryption for Chrome/Edge: read and decrypt the Local State `os_crypt.encrypted_key`, decrypt `v10`/`v11` AES-GCM cookie payloads, preserve the copied-DB behavior, and keep tests deterministic with injected key/decryptor fixtures before trying live browser stores.

## 2026-05-23 Auth Token Validation Slice

Status: backend auth/token validation seam added.

Files changed:

- `LinkVault/linkvault-tauri/src-tauri/src/auth.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/commands.rs`
- `LinkVault/linkvault-tauri/src-tauri/src/lib.rs`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.toml`
- `LinkVault/linkvault-tauri/src-tauri/Cargo.lock`

Implemented in this slice:

- Rust auth model for manual `li_at` validation.
- Deterministic fake-client validation for LinkedIn Learning home fetch behavior.
- Trial/free prompt rejection matching the existing C# `nav__button-tertiary` + `Start free trial` behavior.
- Required `JSESSIONID` handling and `Csrf-Token` derivation from the cookie value.
- Enterprise/library account support seam via `enterpriseProfileHash` extraction and `x-li-identity` header emission.
- Browser token candidate model for Chrome, Edge, and Firefox, including first-valid distinct-token selection.
- Live Tauri command `validate_li_at_token` that returns only session/header metadata and does not persist or echo plaintext `li_at`.
- Reqwest-backed LinkedIn home client for future live validation; deterministic tests do not call LinkedIn.

Validation evidence:

- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 17 tests passed.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.

Security notes:

- SQLite schema still has no plaintext token, cookie, or `li_at` column.
- The Tauri validation command accepts the token as input, uses it for the LinkedIn request, and returns only derived session metadata: CSRF value, enterprise hash, and request headers.

Current next slice:

Port browser cookie import from Chrome, Edge, and Firefox: locate profile cookie databases, copy SQLite DB/WAL/SHM before reading locked browser DBs, extract distinct `li_at` candidates, then feed them through the validated token-selection seam. Keep OS decryption/platform-specific pieces behind testable adapters.

## 2026-05-23 Initial Tauri Scaffold

Status: scaffold created under `LinkVault/linkvault-tauri`.

Files added:

- React/Vite/Tailwind app shell: `LinkVault/linkvault-tauri/src/`
- Local compact primitives: `LinkVault/linkvault-tauri/src/components/primitives.tsx`
- Tauri 2 backend scaffold: `LinkVault/linkvault-tauri/src-tauri/`
- SQLite cache schema: `LinkVault/linkvault-tauri/src-tauri/src/cache.rs`
- LinkedIn URL parser: `LinkVault/linkvault-tauri/src-tauri/src/linkedin.rs`
- Quality fallback seam: `LinkVault/linkvault-tauri/src-tauri/src/quality.rs`
- Safe archive path guard seam: `LinkVault/linkvault-tauri/src-tauri/src/security.rs`

Implemented in this slice:

- Side-by-side Tauri 2 + Rust + React 19/Vite scaffold without changing the existing C# app.
- Tailwind v4 token setup through `@tailwindcss/vite`.
- First-screen downloader UI following `design.md` and `reference.png`: persistent 15rem sidebar, `LinkedIn Courses` route, course setup panel, activity panel, queue panel, compact controls, masked token input, disabled Generic Video row, and 1080p default resolution.
- Tauri commands for bootstrap state, LinkedIn course URL parsing, and quality fallback order.
- SQLite schema for `settings`, `course_cache`, `jobs`, `job_events`, and `artifacts`; no plaintext token/cookie columns.
- Deterministic Rust tests for URL normalization/rejection, blank-line handling, 1080 fallback, safe archive path rejection, and cache schema creation.

Validation evidence:

- `pnpm.cmd install` completed in `LinkVault/linkvault-tauri`.
- `pnpm.cmd build` passed in `LinkVault/linkvault-tauri`.
- `cargo test` passed in `LinkVault/linkvault-tauri/src-tauri`: 10 tests passed.
- `pnpm.cmd tauri build --debug` passed and built `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Desktop render screenshot captured at `1536x1024`: `LinkVault/linkvault-tauri/output/playwright/linkvault-desktop.png`.
- Laptop render screenshot captured at `1280x800`: `LinkVault/linkvault-tauri/output/playwright/linkvault-laptop.png`.
- Narrow render screenshot captured at `390x844`: `LinkVault/linkvault-tauri/output/playwright/linkvault-mobile.png`.
- Initial narrow render overlap was found and fixed by making the mobile header auto-sized and replacing fragile responsive selectors with named classes.

Known local environment note:

- Port `1420` was already occupied by existing `bunx` processes, so the visual check server was started on `http://127.0.0.1:1421`.
- `pnpm install` warned that `esbuild` build scripts were ignored by pnpm approval policy, but the Vite production build still passed.

Current next slice:

Port backend auth/token seams with deterministic fake-HTTP tests: browser token candidate model, manual `li_at` validation, trial prompt rejection, `JSESSIONID`/CSRF behavior, and enterprise `x-li-identity` extraction. Keep plaintext token values out of SQLite.

## 2026-05-23 Tauri Scaffold Path Decision

Owned scaffold path:

- `LinkVault/linkvault-tauri`

Reason:

- Keeps the Tauri 2 migration side-by-side with the existing C# app without deleting or rewriting it.
- Stays inside the requested `LinkVault/` migration area, so no extra external owned path is required.

MVP scaffold decisions:

- `LinkedIn Courses` is the only enabled downloader route.
- `Generic Video` may appear only as disabled visual context and must not expose working downloader behavior.
- Plaintext LinkedIn tokens must not be stored in SQLite. The scaffold will separate token handling from SQLite persistence; future live storage should use OS credential storage or an equivalent secret store, with SQLite holding only non-secret metadata.

## 2026-05-23 Edge-Case And Reference Pass

Status: local harness created under `LinkVault/`.

Files checked:

- `LinkVault/design.md`
- `LinkVault/reference.png`
- `LLCD.CourseExtractor/Extractor.cs`
- `LLCD.CourseExtractor/ExerciseArchiveExtractor.cs`
- `LLCD.CourseContent/Quality.cs`
- `LLCD.LinkVault/MainWindow.cs`
- `LLCD.DownloaderConfig/Config.cs`
- `LLCD.CourseExtractor.Tests/ExtractorDeterministicTests.cs`
- `LLCD.CourseExtractor.Tests/ExerciseArchiveExtractorTests.cs`

Reference facts:

- `reference.png` is `1536x1024`.
- `design.md` requires a dense desktop productivity shell, 100svh layout, persistent 15rem sidebar, 60px headers, compact controls, explicit accessibility coverage, overlay placement rules, and drift governance.
- The attached Image #1 and `reference.png` define the first-screen target: sidebar, LinkedIn Courses header, Course Setup, Activity panel, and Download Queue.

Scope decision:

- The MVP is LinkedIn Learning course download only.
- Generic Video appears in the reference screenshot and current C# app, but it should not be implemented as a working feature in the first Tauri migration.
- If a nav row for Generic Video remains for visual parity, it must be disabled or marked unavailable and must not imply working downloader support.

Current next slice:

Scaffold the Tauri app in a path explicitly recorded here before code generation starts. Recommended path if the user approves broader edits later: `../linkvault-tauri`. Until then, keep planning artifacts inside `LinkVault/`.

## Validation Evidence

Commands run for this pass:

- Listed `LinkVault/` contents.
- Read the first section of `design.md`.
- Located relevant design sections: layout, primitives, accessibility, overlay placement, trigger matrix, governance, reuse checklist.
- Verified `reference.png` dimensions through `System.Drawing`.
- Searched current C# source/tests for 1080 fallback, token validation, exercise extraction, subtitles, and settings persistence.

No application code was changed.
