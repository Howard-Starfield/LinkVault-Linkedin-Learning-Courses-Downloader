# Changelog

## Unreleased

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
