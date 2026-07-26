# Newspaper command responsibility map

Status: recorded before modularization  
Snapshot: `main` at `08fbb8364904a1b23c55227349c7623b1e5e5cba`, with the current uncommitted newspaper feature work preserved  
Primary owner: `apps/desktop/src-tauri/src/newspaper/commands.rs`

## Why this record exists

`commands.rs` currently has 3,481 lines: approximately 2,763 production lines and
718 test lines. It exposes 24 Tauri commands, but it also owns domain workflows,
SQLite access, queue execution, archive migration, reader persistence, events, and
tests. This document records those responsibilities before code moves so the
refactor can be checked for lost behavior and accidental ownership changes.

The target remains a modular monolith. The public Tauri command names and payloads
must not change merely because internal Rust modules are separated.

## Current responsibility inventory

Line ranges refer to the pre-refactor snapshot named above.

| Current range | Current responsibility | Important dependencies | Proposed owner |
| --- | --- | --- | --- |
| 34-100 | `NewspaperState`, queue/cancellation locks, library revision, dimension-backfill scheduling | Tauri managed state, async runtime, SQLite | `state.rs` for state; `page_metadata.rs` for the background worker |
| 101-178 | Bootstrap, catalog listing, remote catalog refresh | `storage`, `catalog`, `reqwest`, catalog SQL | thin wrappers in `commands.rs`; behavior in `catalog_service.rs` |
| 179-407 | Batch validation, date expansion, duplicate detection, batch/job/page creation | models, SQLite, ID/path helpers | `batch_service.rs` |
| 408-494 | Create, toggle, and delete schedules | schedule validation, SQLite | `schedule_service.rs` |
| 495-557 | Queue/optimization command locks, library invalidation event, thumbnail scheduling | `NewspaperState`, Tauri events, `ThumbnailCoordinator` | wrappers in `commands.rs`; event helper in `events.rs` |
| 558-764 | Pause/cancel/retry/reorder/remove batch and job operations | `storage`, SQLite, cancellation state, event history | `job_service.rs` |
| 765-1008 | Legacy library list plus paginated/filterable Library query | SQLite, thumbnail URL construction, reading progress | `library_service.rs` |
| 1009-1033 | Activity snapshot composition | library revision, batch/job/schedule queries | `library_service.rs` |
| 1034-1091 | Reader manifest query and page media URLs | SQLite, thumbnail media URL helper | `reader_service.rs` |
| 1092-1184 | Missing-dimension candidates, batched persistence, WebP header parsing | filesystem headers, SQLite | `page_metadata.rs` |
| 1185-1260 | Reading-progress save/resume/furthest-page rules | SQLite, canonical page ordering | `reader_service.rs` |
| 1261-1284 | Thumbnail command adapter and open-folder adapter | `ThumbnailCoordinator`, opener plugin | remain thin adapters in `commands.rs` |
| 1285-1352 | Import/repair command adapters and post-operation invalidation/prewarming | async blocking pool, events, thumbnails | wrappers in `commands.rs`; behavior in `archive_service.rs` |
| 1353-1811 | Queue loop, job download, retries, validation, page persistence, optimization loop | client, manifest, downloader, optimizer, `storage` | `queue_service.rs` |
| 1812-2019 | Catalog/batch/schedule/job/progress/settings reads and row mapping | SQLite and models | initially `repository.rs`; split only if a concrete boundary warrants it |
| 2020-2135 | Request validation, due-schedule materialization, date/ID/path helpers | models, SQLite, chrono | validation beside its service; shared pure helpers in `naming.rs` only when used by multiple services |
| 2136-2350 | Terminal-state transitions, interrupted/release retry decisions, cross-date guard, progress rollups | SQLite, filesystem, job models | `queue_service.rs` and `job_service.rs` according to caller ownership |
| 2351-2725 | Archive repair, redundant-source cleanup, archive import, identity/page-number parsing | filesystem, optimizer, `storage`, SQLite | `archive_service.rs` |
| 2726-2762 | Batch terminal rollup | SQLite | `job_service.rs` |
| 2763-3481 | Mixed tests for validation, batches, queue controls, reader, schedules, repair, and optimization cleanup | all concerns above | move each test beside the behavior it verifies; keep cross-module integration tests under `newspaper/tests` if needed |

## Public command contract

These names are consumed by `lib.rs` and/or the React frontend and must remain
stable during the refactor.

| Command | Responsibility after refactor |
| --- | --- |
| `bootstrap_newspaper_state` | Compose the initial catalog, batches, jobs, schedules, progress, and settings response |
| `list_newspaper_catalog` | Read the persisted catalog |
| `refresh_newspaper_catalog` | Fetch special editions and upsert the catalog |
| `create_newspaper_batch` | Validate and persist one requested batch and its jobs/pages |
| `create_newspaper_schedule` | Validate and persist a recurring schedule |
| `toggle_newspaper_schedule` | Enable or disable a schedule |
| `delete_newspaper_schedule` | Delete a schedule |
| `process_newspaper_queue` | Enforce the single-run lock and invoke the download queue |
| `process_newspaper_optimization_queue` | Enforce the single-run lock and invoke optimization |
| `pause_newspaper_batch` | Pause or resume all eligible jobs in a batch |
| `cancel_newspaper_batch` | Cancel a batch and active queue work |
| `retry_newspaper_job` | Requeue only missing/failed work without discarding completed pages |
| `set_newspaper_job_pause` | Persist one job's pause state |
| `reorder_newspaper_jobs` | Persist explicit queue order |
| `remove_newspaper_job` | Dismiss/cancel one job while preserving history and files |
| `list_newspaper_library` | Preserve the legacy full-list compatibility surface until all callers are removed |
| `get_newspaper_library_page` | Return filtered, bounded Library metadata |
| `get_newspaper_activity_snapshot` | Return queue/activity state with the library revision |
| `get_newspaper_reader_manifest` | Return page metadata and local media URLs without decoding page files |
| `save_newspaper_reading_progress` | Persist last and furthest canonical reader positions |
| `ensure_newspaper_thumbnail` | Delegate deduplicated thumbnail generation |
| `open_newspaper_download_folder` | Validate a directory and delegate to the opener plugin |
| `import_existing_newspaper_archive` | Register an existing archive, then invalidate and prewarm |
| `repair_newspaper_library` | Repair/optimize registered files, then invalidate and prewarm |

## Dependency direction

The intended direction is:

```text
Tauri command adapters
        |
        v
domain services (batch, job, schedule, queue, library, reader, archive)
        |
        +----> repository helpers ----> SQLite
        |
        +----> existing ports (client, downloader, optimizer, thumbnails, storage)
        |
        +----> pure helpers (page metadata, validation, naming)
```

Services must not invoke Tauri commands. Tauri-specific state extraction, event
emission, and plugin calls stay at the adapter edge or in a narrowly named
infrastructure helper. Pure parsing and validation modules must not depend on
Tauri.

## Behavior invariants

Every extraction must preserve these contracts:

1. All 24 command names, argument casing, serialized response shapes, and frontend
   invocation sites remain unchanged.
2. Only one download/optimization queue owns the shared run lock at a time.
3. Cancellation and pause operations continue to affect active work promptly.
4. Completed pages survive retry, restart reconciliation, repair, and optimization.
5. A job is finalized only after required pages and optimization are terminal.
6. Partial jobs never receive a complete marker.
7. Reader manifest construction performs no full-image decode or page-file scan.
8. Reader media and thumbnails continue through cacheable local protocol URLs.
9. Reading progress preserves both the last page and monotonically increasing
   furthest page.
10. Library invalidation revision/events occur after every mutation that changes
    Library-visible state.
11. Import and repair never delete a source until a valid replacement exists.
12. Schedule materialization remains idempotent for a local publication date.

## Extraction order

The refactor should be behavior-preserving and incremental:

1. Extract `page_metadata.rs`. It is a small, mostly pure seam with focused tests
   and no public command contract.
2. Extract `reader_service.rs`. Keep Tauri wrappers in `commands.rs`; move manifest
   and progress behavior together.
3. Extract `archive_service.rs` and its repair/import tests.
4. Extract `schedule_service.rs`.
5. Extract `library_service.rs` and shared read-only repository mapping.
6. Extract `job_service.rs`.
7. Extract `queue_service.rs` last because it has the widest dependency surface
   and owns cancellation, retries, download, optimization, and terminal-state
   interactions.
8. Reassess whether the remaining repository helpers warrant one module or several;
   do not introduce layers that only forward calls.

After each step, run formatting, focused tests for the moved behavior, all newspaper
tests, `cargo check`, the frontend build, and the newspaper scale checks. Stop the
refactor if a command contract or invariant above cannot be preserved without a
separate design decision.

## Refactor progress

### Completed in the first modularization slice

- `page_metadata.rs` now owns the legacy geometry worker, candidate query, batched
  persistence, decoder-free WebP header parsing, and its focused parser test.
- `reader_service.rs` now owns reader manifests, canonical page indexing, reading
  progress persistence, and progress queries.
- `commands.rs` retains only the corresponding Tauri adapters and state extraction.
- All 24 public command paths remain registered through
  `newspaper::commands::*`.
- `commands.rs` decreased from 3,481 to 3,180 lines without moving queue, archive,
  schedule, or job-control behavior prematurely.

Verification after this slice: 60 newspaper tests passed, `cargo check` and
`cargo fmt -- --check` passed, the TypeScript/Vite production build passed, and
the 8/50/500 newspaper performance contracts passed.
