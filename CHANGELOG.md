# Changelog

## Unreleased

## 0.2.23 - 2026-08-26

- **YouTube helpers install post-setup (~220 MB).** First YouTube use can download the signed Infield media toolchain into `LinkVaultData` instead of bundling helpers in the installer.
- **Dropped ffprobe from the required helper set.** Media verification now uses the bundled `ffmpeg` probe path; the lock and installer expect yt-dlp, Deno, and ffmpeg only, matching toolchain v0.1.1.

## 0.2.22 - 2026-08-26

- **Product rebrand to LinkedVault.** User-facing UI, window title, tray labels, installer `productName`, and docs now say LinkedVault. The left sidebar drops the All-in-One / PNG wordmark for a theme-aware text wordmark. Crate name, exe, bundle identifier, localStorage keys, and the `LinkVaultData` folder are unchanged.
- **Title bar follows the app theme.** Dark and light modes sync the Windows title bar via Tauri `setTheme`.
- **LinkedIn video wait control.** Exposes the random 20–40s (configurable) pause between video downloads beside Quizzes. Changes apply to the next wait; an in-progress wait finishes first.
- **Newspaper Download editions queue owns schedules and history.** Removed the separate Schedule/History panel. Daily schedules (enabled or paused) appear in Queue; finished and failed jobs stay in Completed/Failed. Empty “No schedules yet” placeholder is gone.
- **Tightened newspaper Optimize row layout.** Optimize / Keep JPG size to content so Quality, Workers, and Max sit without dead space.
- **LinkedIn active-row controls.** Pause/resume and delete work for active downloads; expanded course detail flushes under the rounded row without a sharp cutoff.
- **Future scheduled newspaper jobs no longer block the queue.** Immediate work stays first; promoting a future job clears its wait when reordered.
- **Open-source community docs.** Added `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), and `SECURITY.md`, plus README documentation links and current Download / Add schedule wording.
- **Repo hygiene.** Hardened `.gitignore` for `.tmp/`, SQLite wal/shm, and OS junk. Removed unused SVG wordmarks, obsolete one-shot Python icon/version scripts, and unused `SectionHeader` / `SummaryChip` / `ActivityEventRow` primitives with orphan CSS.

## 0.2.19 - 2026-08-19

- **Hardened the desktop shell against sidebar and window resizes.** Resizing the native window or the left navigation rail now keeps the LinkVault wordmark visually fixed, keeps controls inside their layout owners, and reflows provider content from the space it actually receives instead of from viewport assumptions. The LinkVault desktop shell now behaves like a stable desktop application under repeated resizing.
- **Capped the sidebar at its layout owner so it can never scale the wordmark or push content out of its column.** The new `--sidebar-width-cap: 320px` and `--sidebar-effective-width: min(var(--sidebar-width, 220px), var(--sidebar-width-cap))` on `.lv-shell` ensure the rail stops growing once it reaches the cap. The LinkVault wordmark is also capped via `--brand-logo-max-width: 180px` on `.lv-brand-logo`, so widening the sidebar no longer scales the wordmark.
- **Made the live sidebar drag have a single owner.** The imperative `--sidebar-width` mutation during mouse drag no longer races the React `style` prop, so concurrent React/Tauri state updates can no longer snap the rail backward mid-drag.
- **Reflows before any `overflow: hidden` is allowed to mask controls.** New narrow-viewport rules in `index.css` reflow provider content first; controls that no longer fit are no longer hidden behind overflow. Provider views now adapt to the width they actually receive after sidebar and secondary-column allocation, not to the raw viewport.
- **Added a no-explicit-`any` repository gate.** A new AST-based `verify-no-any` script plus the `verify:no-any` npm script fail the build if an explicit TypeScript `any` or `as any` lands in the frontend. Unknown external values now start as `unknown` and are narrowed or decoded, so the no-`any` rule is enforced at the gate level rather than by inspection.
- **Extended the Playwright visual regression sweep to cover the new behavior.** `verify-visual.mjs` now exercises live sidebar dragging, wordmark invariance across the full 208-320 px rail range, LinkedIn/Coursera geometry with a wide sidebar, and concurrent React updates while a drag is in progress.
- **Documented the frontend/Rust ownership boundary.** A new `docs/architecture/frontend-rust-ownership-boundary.md` codifies the split: pointer input, element geometry, focus, responsive state, theme, and other presentation-only concerns live in React/CSS; validation, scheduling, persistence, filesystem work, queue behavior, auth, provider rules, and durable application decisions live in Rust. Indexed from `docs/architecture/README.md`. The full rationale and the G-1..G-6 acceptance criteria are captured in `docs/specs/frontend-responsive-layout-hardening.md`.

## 0.2.15 - 2026-07-31

- **Fixed the v0.2.13-v0.2.14 newspaper upgrade migration.** Existing databases were already marked as schema version 1, so the newly added `newspaper_batches.schedule_id` and `newspaper_schedules.date_mode` provider migrations were skipped even though fresh databases contained both columns. The global schema version now advances to 2, forcing a verified pre-migration backup and running the idempotent provider migrations before newspaper work starts.
- **Added the missing installed-database regression path.** A persistence gate now starts from the released v1 table shape, proves both columns are added, preserves existing batch and schedule rows, defaults legacy schedules to Single date, and confirms the backup retains the untouched v1 schema.

## 0.2.14 - 2026-07-31

- **Added rolling Last 7 days support to daily newspaper schedules.** The saved schedule now preserves the user's date mode and deterministically expands one local-today anchor into the seven calendar dates from today minus six days through today on every poll. Custom date ranges remain manual-only because a fixed historical range is not meaningful for a recurring daily schedule.
- **Kept repeat polls idempotent without hiding incomplete downloads.** A date whose database job is terminal and whose output directory contains the `.complete` marker is skipped on the next seven-day poll. A missing marker remains eligible for repair, and untracked local files are not mistaken for a completed database job. Regression coverage exercises both the rolling window and a second poll over completed database and filesystem state.
- **Simplified the schedule action.** Add another time is now Add schedule and sits beside Download now, while saved schedule cards show whether they use Single date or Last 7 days.

## 0.2.13 ? 2026-07-31

- **Fixed daily newspaper schedules retaining control of queued retries after deletion.** Schedule-created batches now persist their originating schedule identity. Removing a schedule atomically cancels only its queued, active, or optimizing work, clears pending release-retry deadlines, preserves completed newspaper history, and signals an in-flight worker to stop at its safe boundary.
- **Restored immediate manual recovery without queue deletion.** A manual download for the same edition and date can now resume the cancelled schedule-owned job immediately instead of inheriting its future retry time. The delete confirmation explains that pending automatic retries will stop, and regression coverage locks the complete schedule-delete-to-manual-download flow.

## 0.2.12 — 2026-07-29

- **Newspaper schedule card hover polish.** The daily-schedule cards in the World Journal panel now replace the "Enabled / Paused" status badge with the pause / delete action buttons on hover, instead of stacking the badge and the buttons in two different positions. The crossfade is a 140ms ease; on touch devices the badge and the buttons sit side-by-side so the controls stay reachable without hover. Reduced-motion users get an instant swap.
- **App icon regen via `generate-app-icons.py`.** Bundled a new script under `apps/desktop/scripts/` that produces the full Tauri 2 icon set (`icon.png`, `icon.ico`, `icon-taskbar.png`, `icon-tray.png`) from a single source PNG, deriving a real alpha channel from the near-white background so the installer / taskbar / tray icons show transparent corners on both light and dark backgrounds. The current v0.2.12 binary ships the regenerated set; downstream releases can rerun the script instead of hand-editing individual icon files.

## 0.2.11 — 2026-07-28

- **Rebrand: All-in-One Downloader.** The left-sidebar banner, the Windows taskbar icon, the system tray icon, and the Windows installer icon are now the "All-in-One Downloader" artwork. This re-introduces the v0.2.8 brand on purpose: the v0.2.10 "LinkVault Course Downloader" wordmark was a generic placeholder and is removed again. `App.tsx` imports the new sidebar asset via `<img src>`; `lib.rs` decodes the taskbar icon from `include_bytes!` so the binary has no runtime file dependency.
- **New system tray icon** with a right-click menu. The app now stays reachable from the Windows notification area when the main window is closed. The tray carries a `Show` entry (un-hides, un-minimizes, and focuses the main window) and a `Quit` entry (exits the app). Left-click on the tray does **not** open the menu — the menu is right-click only, to avoid stealing focus from the user's current activity. The Tauri `tray-icon` feature is enabled in `Cargo.toml`.
- **Restored the v0.1.3 red-highlighted F12 cookie guide.** Replaces the v0.2.10 Chrome DevTools capture in the "Find your LinkedIn li_at cookie" dialog. The v0.1.3 image highlights the F12 → Application tab → Cookies → `li_at` row flow with red callouts and big "Press F12 on Linkedin Learning" text, which is the version users actually found helpful. The v0.2.10 capture is the one that's now removed.
- **Quantized the large icon and banner PNGs** to a 128-color palette with median-cut + Floyd-Steinberg dithering. The 1024×1024 master `icon.png` dropped 1.4% (the source already had a compact palette), and the 1264×424 `linkvault-wordmark.png` banner dropped 25.7% (576 KB down from 776 KB) with no visible banding. Both files now load faster and the binary bundle is ~200 KB lighter.
- Bundled icon set in `apps/desktop/src-tauri/icons/` is now the full Tauri 2 set: `icon.png` (1024×1024, Linux/macOS bundle), `icon.ico` (Windows installer, multi-res 16/24/32/48/64/128/256), `icon-taskbar.png` (48×48, window icon), and the new `icon-tray.png` (48×48, system tray).

## 0.2.10 — 2026-07-28

- Restored the LinkVault Course Downloader wordmark in the left sidebar. The v0.2.8 swap to a generic "All-in-One Downloader" graphic is reverted: the brand area now renders the cropped LinkVault SVG (viewBox `0 10 470 95`, 19% shorter than the v0.2.7 SVG) so the top section sits tighter against the panel. The wrong-brand `linkvault-wordmark.png` has been removed from the repository; `App.tsx` imports the SVG via `<img src>`, which honours the existing `.lv-brand-logo img { width: 100%; height: auto; }` rule. Users still on v0.2.8 or v0.2.9 will see the correct LinkVault wordmark after the in-app updater pulls this release.
- Replaced the LI-AT cookie guide screenshot in the "Find your LinkedIn li_at cookie" dialog with the correct Chrome DevTools capture: Application tab open, Storage → Cookies → `www.linkedin.com` selected on the left, with the `li_at` cookie row visible in the cookie list. The previous image did not match the dialog's alt text and would have misled users about which cookie to copy.
- Polished the left sidebar trigger button: the panel-collapse handle is now hidden by default and reveals itself on hover with a soft fade-in (160 ms), matching the rest of the rail's surface treatment. A `focus-within` rule keeps it visible for keyboard users.

## 0.2.9 — 2026-07-28

- Newspaper image optimization now starts as soon as the first edition finishes downloading, instead of waiting for the entire batch. The per-edition trigger fires inside the download worker the moment a job reaches a `completed` or `partial` terminal status, so a "last 7 days" submission starts optimizing day 1 while days 2-7 are still downloading. The optimization queue and the download queue are now driven by independent `download_running` and `optimization_running` flags so they can overlap; the shared cooperative `cancelled` flag still stops both at safe boundaries.
- The optimization governor's auto mode now targets a 50% CPU cap and re-evaluates the admitted worker pool every 3 seconds (down from every 1 second) to avoid oscillating around the cap. Manual mode still respects the user-configured worker ceiling and the new memory knobs as the upper bound.
- Added two new governor knobs in the Newspaper section of the Settings dialog: **Memory per worker (MB)** (default 160, range 64-1024) and **Memory reserve (MB)** (default 4096, range 512-32768). 4K and other memory-hungry editions can raise the per-worker budget to avoid swap pressure; raising the reserve leaves more headroom for the rest of the OS, the LinkVault UI, and any active download. Both values are clamped server-side to safe bounds and persisted to `linkvault.newspaper.optimizationPreferences` in localStorage.
- The "LinkedIn Scraper / Coming soon" placeholder in the left sidebar has been replaced with a live **Optimization** status panel that subscribes to the `newspaper://optimization-progress` event and renders the admitted worker count, the number of active workers, and the live system CPU percent. When nothing is optimizing, the panel shows an "Idle" pill instead. Regression coverage locks the eligible-status allowlist for the per-edition trigger and the new governor threshold constants.

## 0.2.8 — 2026-07-27

- Rebuilt the left sidebar so the three expandable provider buttons (LinkedIn Courses, Coursera Courses, World Journal) no longer carry the active highlight, and added provider-specific Download LinkedIn and Download Coursera children as the first item under their groups, mirroring the World Journal's Download editions child. Clicking a parent now expands and snaps to its first child; collapsing leaves the current child active.
- Added an in-app update banner that surfaces a sticky theme-aware strip across the top of the window whenever the Tauri updater reports a new LinkVault release, with a one-click install action and a dismiss button that automatically re-appears when a fresh update is detected.
- Refreshed the LinkVault wordmark and the in-app guide image, and regenerated the Tauri application icon set (icon.ico, icon.png, icon-taskbar.png) for a substantially smaller and crisper build.
- Fixed the Reset World Journal database action so it no longer wipes the built-in newspaper edition catalog. v0.2.7 was clearing `newspaper_editions` along with user data, but the 13 built-in editions (10 daily regions plus 3 weeklies) and any previously-discovered specials are application-owned data the user cannot edit through the UI, and `seed_built_in_catalog` only runs once at database initialization. After a reset, the regions (dailies like NY, LA, SF, …) would only come back if the world journal site happened to surface them, which it does not, so the Regional tab ended up empty.
- The fix removes the `DELETE FROM newspaper_editions` from `clear_newspaper_provider_data` and drops the now-unused `editions` field from `NewspaperResetCounts`. The wipe now removes only user and derived data (jobs, batches, pages, thumbnail cache, optimization tasks, reading progress, schedules, events, settings). The LinkedIn sentinel job and the shared `download.folder` setting continue to survive.
- Added a startup self-heal that recovers any v0.2.7 installation whose user already wiped the catalog. `newspaper::storage::ensure_catalog_populated` runs on every Tauri startup, counts the built-in catalog rows (those with an empty `publication_date`), and re-seeds them when the count is zero. Fresh databases and intact v0.2.7 installations hit the no-op path (one `SELECT COUNT(*)`); v0.2.7 installations whose users clicked Reset World Journal database get the 13 built-in editions restored on the next app launch with no user action required. The recovered editions are reported through the existing `database_diagnostics` stream as an `ensure_newspaper_catalog_populated` event.
- Added two regression tests: one confirms `ensure_catalog_populated` is a no-op when the built-in catalog is intact, and one simulates the v0.2.7 empty-catalog state and confirms the self-heal restores all 13 built-in editions. The second test also documents the boundary: discovered specials that the v0.2.7 reset wiped are not auto-restored (the next `refresh_newspaper_catalog` call re-discovers them), so the self-heal is deliberately scoped to the built-in catalog only.

## 0.2.7 — 2026-07-27

- Added a Data management section in Settings with destructive reset actions for the LinkedIn, Coursera, and World Journal provider databases. Each button opens a confirmation dialog that lists exactly which tables will be cleared and confirms that downloaded files on disk are not touched.
- Added `reset_linkedin_database`, `reset_coursera_database`, and `reset_newspaper_database` Tauri commands that wipe the provider's tables in a single transaction through `app::cache` helpers, reset the provider's in-memory state (cancelled flag, running flag, library and progress revisions), and regenerate the LinkedIn Markdown history file as empty.
- The reset flow first invokes the existing bulk-pause mechanism (`set_all_downloads_paused` for LinkedIn, `cancel_active_coursera_download` for Coursera, `set_all_newspaper_jobs_paused` for World Journal) so any in-flight worker unwinds at a safe boundary, then waits 1.5 seconds before the wipe commits. The reset commands also defensively re-arm the cooperative cancellation flag in case the worker has already exited.
- The World Journal reset additionally wipes the on-disk `newspaper-thumbnails/` directory using the same canonicalize + starts_with safety pattern as `remove_cached_thumbnail`, and surfaces an Open output folder action on the success toast.
- The LinkedIn `li_at` cookie, the `download.folder` shared setting, and any cross-provider tables are explicitly preserved by every reset.
- Added regression coverage in `app::database` and `newspaper` tests that the wipe is strictly scoped to the targeted provider's tables and never touches the schema, the shared `settings` row, or another provider's data.
- Bumped the persistence-legacy-write baseline to v3 to record the new shared write helper call sites.

## 0.2.6 — 2026-07-27

- Added a Pause all / Resume all control that morphs into the primary Download-now slot while newspaper downloads are active or queued, then reverts to Download now once the queue drains.
- Added a `set_all_newspaper_jobs_paused` Tauri command that flips every visible queued, active, and optimizing job in a single transaction and signals the shared cooperative cancellation flag only when an in-flight download is affected.
- Cleared the cooperative cancellation flag on resume so a follow-up queue pass is guaranteed to re-arm the worker, even if the previous worker has already unwound at a safe boundary.
- Added regression coverage that the bulk pause skips terminal and dismissed jobs, mirrors the per-job `active → queued` safe-boundary transition, and is a no-op when the queue is already drained.

## 0.2.5 — 2026-07-27

- Fixed Last 7 days runs that appeared to stall on an unavailable current edition by restoring legacy dismissed editions that are already downloaded and requeueing editions whose completed files are missing.
- Changed the Progress trash action into a confirmed permanent deletion that removes the exact inactive edition folder, generated thumbnail, progress history, and duplicate identity so the edition can be downloaded again.
- Added deletion safety guards for active downloads and paths outside the configured newspaper destination, with real SQLite and temporary-filesystem regression coverage.

## 0.2.4 — 2026-07-27

- Superseded 0.2.3 with a distinct version so installations that previously identified themselves as 0.2.3 can discover and install the audited architecture and persistence release through the updater.

## 0.2.3 — 2026-07-27

- Established explicit `app`, `workflow`, and provider ownership boundaries so LinkedIn Learning, Coursera, Newspaper, and future providers can share one durable workflow architecture without duplicating schedulers or job engines.
- Added versioned SQLite startup migrations with verified, non-overwriting pre-migration backups, a consistent WAL/runtime connection policy, and future-schema rejection.
- Added a dedicated serialized database writer with graceful shutdown draining, panic containment, independent WAL readers, and bounded redacted diagnostics.
- Added structural architecture and persistence gates that freeze legacy provider write growth while provider cutovers proceed incrementally.
- Recorded Windows release contention and native migration evidence, including 800 concurrent writes with zero failures and preservation of pre-migration Newspaper data.

## 0.2.2 — 2026-07-26

- Reworked the newspaper reader into a wider, seamless canvas with a slim one-row toolbar, immediate pointer-centered zoom, persistent default/click zoom preferences, and accurate unique-page reading progress.
- Moved newspaper reader and archive maintenance controls into Settings, leaving the Library toolbar compact and revealing optimization guidance only on hover.
- Added a durable page-level optimization ledger with crash recovery, lease reconciliation, safe original preservation, and exact download/optimization progress.
- Added a CPU- and memory-aware conversion swarm with Auto mode, manual ceilings up to 20 workers, throttled runtime feedback, and 500-page stress coverage.
- Expanded UI, browser, performance, recovery, and backend verification for newspaper reading and optimization.

## 0.2.1 — 2026-07-26

- Modularized the newspaper backend behind its stable 24-command Tauri facade, moving state, catalog, batch, schedule, Library, job lifecycle, queue, archive, optimization, naming, and repository ownership into focused modules while preserving behavior and reducing `commands.rs` from 3,481 to 303 lines.
- Updated PostCSS and NanoID to patched releases after the pre-release dependency audit identified a high-severity source-map path traversal advisory.
- Rebuilt Newspaper Downloads as a compact three-panel dispatch board that keeps the LinkVault sidebar, removes decorative step numbers and panel headings, orders Editions → Download settings → Schedule, groups regional/weekly/special editions, moves the save location into Download settings, reuses the compact LinkedIn action buttons, reserves a collision-free Progress area, and suppresses expensive transitions and repaints while the desktop window is actively resizing.
- Added persisted newspaper queue ordering, row-level pause/resume, safe removal that leaves downloaded files on disk, timestamped download history, hover-revealed actions, and restart reconciliation for paused or dismissed work.
- Fixed World Journal page filenames by deriving the image extension from the embedded page URL instead of the PHP delivery endpoint.
- Added an in-app repair action that renames legacy PHP-suffixed images and applies the selected WebP optimization profile.
- Fixed last-seven-days batches to use the computer's current local date and skip already-known editions instead of failing on duplicate jobs.
- Changed the default inter-edition delay to 15 seconds and added page-level real-time progress.
- Added unreleased-edition detection with automatic 30-minute retries.
- Replaced one-time scheduling with a recurring daily local-time schedule that skips editions already present in the library.
- Separated recurring schedule creation from immediate date-range downloads so Last 7 days always queues all seven dates.
- Moved image optimization into a resumable post-download queue so WebP encoding no longer blocks the next edition.
- Hardened `.complete` so it is written only after every recorded page exists at its validated size and any requested optimization has finished; stale database rows now requeue missing files automatically.
- Replaced the two fixed WebP profiles with a compact adjustable compression-strength gauge spanning quality 55–92 while preserving the original page dimensions.
- Rebuilt the newspaper reader as a full-window canvas with back navigation, direct 20% click zoom, a 50–300% zoom ruler, keyboard controls, and responsive header navigation.

## 0.2.0 — 2026-07-24

- Added a World Journal provider with compact edition selection and a row-based newspaper library.
- Added daily, Sunday-weekly, and live-discovered special publication catalogs.
- Added single-date, last-seven-days, custom-range, scheduled, and delayed batch downloads.
- Added validated atomic page downloads, restart recovery, partial completion, retry-missing, pause, and cancellation.
- Added high-clarity WebP 92 and balanced WebP 86 optimization with original-file safety.
- Added shallow A01 previews, an offline page reader, folder actions, and existing archive registration.
- Added expandable LinkedIn and Coursera history navigation plus bottom settings and light/dark theme controls.
- Restored UI, release-manifest, Tauri smoke, installer, and release verification scripts.
