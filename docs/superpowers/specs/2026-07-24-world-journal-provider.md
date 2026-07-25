# World Journal Provider

## Metadata

- **Author:** Codex with Howard
- **Date:** 2026-07-24
- **Status:** Approved
- **Reviewer:** Howard
- **Target:** LinkVault desktop application
- **Reference implementation:** `C:\Users\howard\Downloads\Ai_script\Newspaper Extractor`
- **Approved visual reference:** compact, cardless two-column downloader and row-based newspaper library

## Context

LinkVault currently downloads LinkedIn Learning and Coursera courses through separate provider surfaces. The user also maintains a standalone World Journal Newspaper Extractor containing a Python prototype, a stronger Rust rewrite, and an existing archive of roughly 27,000 downloaded page images. The standalone Rust implementation proves the World Journal manifest pattern, edition schedules, required request headers, retry behavior, cancellation, page hashing, and Windows path handling.

The newspaper workflow belongs in LinkVault as an isolated third provider. It must reuse the LinkVault shell, expandable provider navigation, theme, status language, scheduling concepts, and history density without mixing newspaper page semantics into course jobs or artifacts.

The approved downloader UI is intentionally compact. It has no redundant page title, no headings above its two setup columns, no nested section cards, and no console. Edition selection occupies the left column, download options occupy the aligned right column, scheduling is a full-width action row below them, and active downloads use a fixed-column table. The library uses shallow masthead/first-headline previews in divider-separated rows rather than cards or full-page covers.

The feature must correct known weaknesses in the standalone implementation. A partial edition must never receive a completion marker, restarts must resume validated pages safely, scheduling must survive app restarts, and optional image optimization must preserve originals until replacements have been validated.

## Functional Requirements

### Provider and navigation

- FR-1: LinkVault MUST expose a World Journal provider group with `Download editions` and `Newspaper library` child views.
- FR-2: Newspaper frontend and backend behavior MUST be isolated from LinkedIn and Coursera job models, commands, cancellation state, and persistence.
- FR-3: The downloader view MUST NOT display a redundant page title or headings above its edition-selection and download-options columns.
- FR-4: The downloader view MUST use a cardless two-column setup region, one full-width schedule/action row, and a fixed-column active-download table.
- FR-5: The library MUST use divider-separated rows and MUST NOT use independent content cards.

### Catalog and selection

- FR-6: The system MUST provide a built-in catalog of supported daily and weekly editions when live catalog discovery is unavailable.
- FR-7: The system SHOULD discover current special publications from the official World Journal e-paper site and merge them with the built-in catalog without removing built-in editions.
- FR-8: Users MUST be able to search editions and filter Daily, Weekly, and Special editions.
- FR-9: Users MUST be able to select individual editions or all daily editions.
- FR-10: Users MUST be able to choose a single publication date, the last seven days, or a custom date range.
- FR-11: Last-seven-days jobs MUST be generated oldest-first.
- FR-12: Weekly editions MUST create jobs only on dates valid for their publication schedule.
- FR-13: A custom date range MUST NOT exceed 31 days.

### Scheduling and delay

- FR-14: Users MUST be able to choose immediate or scheduled execution through one `Schedule download` checkbox.
- FR-15: When scheduling is disabled, date/time controls MUST be hidden and the primary action MUST read `Download now`.
- FR-16: When scheduling is enabled, date/time controls MUST be visible and the primary action MUST read `Schedule downloads`.
- FR-17: Scheduled batches MUST persist as an absolute UTC timestamp.
- FR-18: Scheduled batches MUST execute while LinkVault is running.
- FR-19: If LinkVault was closed when a batch became due, the overdue batch MUST begin after the next launch and MUST record an overdue-start event.
- FR-20: Users MUST be able to configure a delay from 0 through 1,440 minutes between edition jobs.
- FR-21: Delay MUST occur after a non-final edition job and before the next edition job.
- FR-22: Delay MUST be pause-aware and cancellation-aware.

### Downloading and recovery

- FR-23: The downloader MUST send the configured User-Agent and the edition-specific Referer on manifest and page requests.
- FR-24: The manifest parser MUST reject non-JSON content types, HTML bodies presented as JSON, malformed JSON, and manifests with no pages.
- FR-25: HTTP 404 for an edition manifest MUST produce `unavailable`, not `failed`.
- FR-26: Transient request failures and HTTP 5xx responses MUST retry after 1, 3, and 9 seconds.
- FR-27: HTTP 4xx responses other than explicitly classified availability responses MUST NOT retry.
- FR-28: Pages within one edition SHOULD download concurrently with a bounded default limit of four.
- FR-29: Only one edition job in a batch MUST be active at a time so inter-edition delay remains deterministic.
- FR-30: Page bytes MUST be written to a sibling `.part` path before validation and atomic promotion.
- FR-31: A downloaded page MUST be image-decodable before it can transition to completed.
- FR-32: A page checksum and byte size MUST be stored after validation.
- FR-33: An edition MUST NOT transition to completed or receive `.complete` if any required page is incomplete or failed.
- FR-34: An edition with at least one successful page and at least one failed required page MUST transition to `partial`.
- FR-35: On restart, active edition jobs MUST become queued, active page records MUST become pending, validated completed files MUST be skipped, and incomplete `.part` files MUST restart.
- FR-36: Users MUST be able to pause, resume, cancel, and retry newspaper work without affecting other providers.
- FR-37: A first-page hash matching a different publication date MUST create a possible-placeholder warning but MUST NOT automatically discard the explicit user request.

### Image optimization and preview

- FR-38: Users MUST be able to enable or disable post-download image optimization.
- FR-39: The initial optimization profiles MUST be `High clarity · WebP 92` and `Balanced · WebP 86`. `High clarity · WebP 92` MUST be the default. Lossless WebP MUST NOT be exposed because the Phase 0 benchmark found that it increased representative JPEG newspaper pages to roughly 4.2 times their source size.
- FR-40: Optimization MUST preserve pixel dimensions.
- FR-41: Optimized output MUST be written to `.part`, decoded successfully, and atomically promoted before it is accepted.
- FR-42: If optimized output is not smaller than the source, the system MUST retain the source as the final page.
- FR-43: When `Keep original JPG files` is disabled, originals MUST remain until all pages in the edition have optimized successfully.
- FR-44: Optimization failure MUST preserve the original page and MUST produce a warning rather than fail an otherwise complete download.
- FR-45: The system MUST generate a small shallow preview from the top of A01 that contains the masthead and first headline.
- FR-46: Preview generation failure MUST NOT invalidate an otherwise complete edition.

### Library and reader

- FR-47: The library MUST display newest-first results with search, edition-kind filtering, status filtering, and incremental paging.
- FR-48: Each library row MUST display the shallow A01 preview, edition names, code, publication date, page count, status, and actions.
- FR-49: The library MUST NOT display an entire newspaper page as the row thumbnail.
- FR-50: Users MUST be able to open a completed or partial edition in a local reader.
- FR-51: The reader MUST support previous page, next page, page selection, zoom in, zoom out, fit width, and keyboard navigation.
- FR-52: Users MUST be able to open an edition folder in the OS file explorer.
- FR-53: Partial editions MUST expose a retry-missing-pages action.
- FR-54: Users SHOULD be able to register an existing Newspaper Extractor archive without moving or redownloading files.
- FR-55: Existing archive registration MUST NOT optimize or delete existing pages.

### Settings and compatibility

- FR-56: Destination, delay, selected filters, optimization profile, keep-original preference, and scheduling preference MUST persist across restarts.
- FR-57: Browser-preview fixtures MUST support the downloader, active states, failure states, and library without a live Tauri runtime.
- FR-58: LinkedIn and Coursera behavior MUST remain unchanged.

## Non-Functional Requirements

- **NFR-1 — Layout:** At desktop widths, the edition and option columns MUST share top and bottom coordinates within one CSS pixel in automated geometry checks.
- **NFR-2 — Density:** Desktop control rows SHOULD use a 36-pixel control height and an 8-pixel spacing grid.
- **NFR-3 — Responsiveness:** At widths below 980 pixels, the setup columns MAY stack, but controls MUST remain usable without horizontal page scrolling.
- **NFR-4 — Accessibility:** All controls MUST be keyboard accessible, have programmatic labels, visible focus, and status text that does not depend only on color.
- **NFR-5 — UI responsiveness:** Scheduling, downloading, hashing, decoding, optimization, and archive scans MUST NOT block the webview UI thread for more than 100 milliseconds.
- **NFR-6 — Memory:** Optimization MUST process at most one full-resolution page per worker, and the default optimization worker count MUST be one.
- **NFR-7 — Network load:** The default per-edition page concurrency MUST be four and MUST NOT exceed eight through supported settings.
- **NFR-8 — Persistence:** Batch, job, and page state transitions MUST be durable before UI success is reported.
- **NFR-9 — Security:** The webview MUST NOT receive broad arbitrary filesystem read permission solely to display newspaper previews.
- **NFR-10 — Security:** Network requests MUST be constrained to configured World Journal HTTPS origins unless an advanced source setting is explicitly introduced in a later spec.
- **NFR-11 — Data safety:** No original page may be deleted until a replacement has passed decode, dimension, and atomic-write validation.
- **NFR-12 — History scale:** Loading the initial library view MUST fetch no more than 50 rows; additional rows MUST be loaded incrementally.
- **NFR-13 — Quality:** A lossy optimization profile MUST pass manual readability review at 100%, 150%, and 200% zoom on at least 20 representative pages before being enabled by default.
- **NFR-14 — Compatibility:** Existing LinkedIn and Coursera automated suites MUST pass without behavior changes.
- **NFR-15 — Windows:** Paths MUST preserve Chinese characters, strip invalid Windows filename characters, and support long absolute paths.

## Acceptance Criteria

### AC-1: Cardless aligned desktop layout (FR-1, FR-2, FR-3, FR-4, FR-5, NFR-1)

Given the World Journal downloader at desktop width, when geometry is measured, then no redundant page/column heading is rendered and both setup columns share top and bottom coordinates within one pixel.

### AC-2: Immediate action state (FR-14, FR-15, FR-16)

Given scheduling is unchecked, when the user views the action row, then date/time controls are absent and the stable primary action reads `Download now`.

### AC-3: Scheduled action state (FR-14, FR-15, FR-16)

Given scheduling is checked, when the user views the action row, then local date/time controls are present and the same primary action reads `Schedule downloads`.

### AC-4: Persisted and overdue schedules (FR-17, FR-18, FR-19)

Given a future scheduled batch is persisted, when the app remains open past its due time, then the batch starts; when the app was closed, then it starts after the next launch and records an overdue-start event.

### AC-5: Delay between editions (FR-20, FR-21, FR-22, FR-28, FR-29)

Given two edition jobs and a five-minute delay, when the first completes, then the second begins no earlier than five minutes later unless the delay is paused or cancelled.

### AC-6: Manifest classification (FR-23, FR-24, FR-25)

Given a fake World Journal server, when manifests return valid JSON, HTML placeholders, malformed JSON, empty pages, or 404, then each response is classified according to the specified manifest and availability rules.

### AC-7: Retry classification (FR-26, FR-27)

Given transient failures, 5xx, and 404 page responses, when a page is requested, then transient/5xx responses retry on the specified schedule and 404 is requested only once.

### AC-8: Validated page persistence (FR-30, FR-31, FR-32)

Given a page download, when it succeeds, then the promoted file is image-decodable and its checksum and byte size are persisted.

### AC-9: Partial edition completion (FR-33, FR-34)

Given one required page fails, when the edition finishes, then status is partial and no `.complete` marker exists.

### AC-10: Restart recovery (FR-35)

Given a process stops during an edition, when LinkVault restarts, then completed valid pages are skipped and pending/partial pages restart.

### AC-11: Optimization fallback (FR-38, FR-39, FR-40, FR-41, FR-42)

Given optimization is enabled, when output validates and is smaller, then it becomes final; when it is not smaller or does not validate, then the source remains final.

### AC-12: Original retention (FR-43, FR-44, NFR-11)

Given keep-original is disabled, when any page optimization fails, then no original in that edition is deleted and the edition records an optimization warning.

### AC-13: Shallow library preview (FR-45, FR-46, FR-48, FR-49)

Given a completed edition, when the library loads, then it shows a shallow A01 masthead/headline preview; if preview generation failed, then the row remains usable with a fallback.

### AC-14: Paged newest-first library (FR-47, NFR-12)

Given more than 50 library editions, when the library opens, then no more than 50 are returned and subsequent pages retain newest-first ordering and active filters.

### AC-15: Offline reader (FR-50, FR-51)

Given a local edition, when the user operates reader controls and arrow keys, then page and zoom state update without network access.

### AC-16: Weekly date expansion (FR-12)

Given a Sunday-only edition and a seven-day range, when jobs are expanded, then exactly the valid Sunday date is included.

### AC-17: Catalog fallback (FR-6, FR-7)

Given live catalog discovery fails, when the catalog loads, then all built-in regular editions remain selectable and a non-blocking refresh warning is recorded.

### AC-18: Existing archive registration (FR-54, FR-55)

Given an existing Newspaper Extractor folder, when it is registered, then edition/date rows are created without moving, rewriting, optimizing, or deleting existing page files.

### AC-19: Provider isolation (FR-36, FR-58, NFR-14)

Given newspaper work is paused or cancelled, when LinkedIn or Coursera state is inspected, then their cancellation flags and jobs are unchanged and their test suites still pass.

### AC-20: Browser-only preview states (FR-57)

Given browser-only Vite preview mode, when newspaper fixture states are selected, then downloader, scheduled, active, partial, failed, and library surfaces render without Tauri commands.

### AC-21: Edition/date selection and preference persistence (FR-8, FR-9, FR-10, FR-13, FR-56)

Given the downloader settings are changed, when the user selects individual or all-daily editions, applies edition filters, chooses a valid date mode, and restarts LinkVault, then the supported preferences are restored; when a custom range exceeds 31 days, then submission is rejected before jobs are created.

### AC-22: Duplicate-cover warning (FR-37)

Given A01 matches the stored hash for another publication date, when the edition runs, then the requested download continues and a possible-placeholder warning is persisted.

### AC-23: Folder and partial retry actions (FR-52, FR-53)

Given a completed or partial library entry, when Open folder is invoked, then the edition directory opens; when Retry missing pages is invoked on a partial entry, then only incomplete or invalid page records return to pending.

## Edge Cases

- EC-1: Manifest returns HTTP 404.
- EC-2: Manifest content type is HTML.
- EC-3: Manifest claims JSON but begins with HTML.
- EC-4: Manifest JSON is malformed or has no pages.
- EC-5: Page response is 404, 5xx, truncated, or not image-decodable.
- EC-6: A01 hash matches a different requested date.
- EC-7: A weekly edition is selected for an invalid date.
- EC-8: A scheduled local time is in the past, ambiguous, or nonexistent because of daylight-saving time.
- EC-9: LinkVault exits before or during a scheduled batch, inter-edition delay, page request, or optimization.
- EC-10: Destination disappears or becomes read-only.
- EC-11: Disk fills during page download, preview generation, or optimization.
- EC-12: Optimized output is larger than its source.
- EC-13: Optimized output decodes but dimensions differ.
- EC-14: Preview generation fails.
- EC-15: Database is unavailable or corrupt.
- EC-16: Existing archive contains Chinese names, long paths, missing `.complete`, mixed image formats, or corrupted pages.
- EC-17: Two batches request the same edition and date.
- EC-18: Live catalog discovery introduces an unknown edition type or changes a code.
- EC-19: User cancels or pauses during the inter-edition delay.
- EC-20: Library preview is requested for an unavailable or deleted local file.
- EC-21: Custom range exceeds 31 days or produces no valid edition/date pairs.

## API Contracts

LinkVault exposes Tauri IPC rather than a network HTTP API. For contract notation, each command is conceptually invoked as `POST /tauri-ipc/newspaper/{command}` inside the desktop process; this is not a listening HTTP endpoint and MUST NOT be exposed on a network port.

```ts
type EditionKind = "daily" | "weekly" | "special";
type BatchStatus =
  | "queued"
  | "scheduled"
  | "active"
  | "paused"
  | "completed"
  | "completed_with_warnings"
  | "failed"
  | "cancelled";
type JobStatus =
  | "queued"
  | "active"
  | "optimizing"
  | "completed"
  | "partial"
  | "unavailable"
  | "failed"
  | "cancelled";
type PageStatus =
  | "pending"
  | "downloading"
  | "downloaded"
  | "optimizing"
  | "completed"
  | "failed"
  | "cancelled";
type OptimizationProfile = "webp_high" | "webp_balanced";

interface NewspaperEdition {
  code: string;
  nameZh: string;
  nameEn: string;
  kind: EditionKind;
  schedule: "daily" | "weekly_sunday" | "ad_hoc";
  sourceUrl: string;
  discovered: boolean;
}

interface CreateNewspaperBatchRequest {
  editionCodes: string[];
  dateMode: "single" | "last_7_days" | "custom";
  startDate: string;
  endDate?: string;
  destination: string;
  scheduledAt?: number;
  delayMinutes: number;
  optimizeImages: boolean;
  optimizationProfile: OptimizationProfile;
  keepOriginalJpg: boolean;
}

interface CreateNewspaperBatchResponse {
  batch: NewspaperBatch;
  jobs: NewspaperJob[];
}

interface NewspaperLibraryRequest {
  query?: string;
  kinds?: EditionKind[];
  statuses?: JobStatus[];
  offset: number;
  limit: number;
}

interface NewspaperLibraryPage {
  items: NewspaperLibraryEntry[];
  total: number;
  offset: number;
  limit: number;
}

interface NewspaperPreviewResponse {
  mimeType: "image/jpeg";
  dataBase64: string;
}

interface NewspaperReaderManifest {
  job: NewspaperJob;
  pages: Array<{
    pageNumber: string;
    section?: string;
    displayPath: string;
    status: PageStatus;
  }>;
}

interface NewspaperError {
  code:
    | "invalid_request"
    | "invalid_schedule"
    | "catalog_unavailable"
    | "manifest_unavailable"
    | "manifest_invalid"
    | "network"
    | "filesystem"
    | "database"
    | "cancelled";
  message: string;
  retryable: boolean;
}
```

Tauri command surface:

```text
bootstrap_newspaper_state
list_newspaper_catalog
create_newspaper_batch
pause_newspaper_batch
resume_newspaper_batch
cancel_newspaper_batch
retry_newspaper_job
list_newspaper_library
get_newspaper_reader_manifest
get_newspaper_preview
open_newspaper_download_folder
import_existing_newspaper_archive
save_newspaper_settings
```

## Data Models

### `newspaper_editions`

| Field | Type | Constraints |
|---|---|---|
| code | TEXT | Primary key |
| name_zh | TEXT | Not null |
| name_en | TEXT | Not null |
| kind | TEXT | daily, weekly, special |
| schedule | TEXT | daily, weekly_sunday, ad_hoc |
| manifest_key | TEXT | Not null |
| source_url | TEXT | HTTPS |
| active | INTEGER | Boolean, not null |
| discovered | INTEGER | Boolean, not null |
| discovered_at | INTEGER | Nullable |
| updated_at | INTEGER | Not null |

### `newspaper_batches`

| Field | Type | Constraints |
|---|---|---|
| id | TEXT | Primary key |
| status | TEXT | BatchStatus |
| destination | TEXT | Not null |
| scheduled_at | INTEGER | Nullable UTC timestamp |
| delay_minutes | INTEGER | 0 through 1440 |
| optimize_images | INTEGER | Boolean |
| optimization_profile | TEXT | OptimizationProfile |
| keep_original_jpg | INTEGER | Boolean |
| created_at | INTEGER | Not null |
| updated_at | INTEGER | Not null |
| completed_at | INTEGER | Nullable |

### `newspaper_jobs`

| Field | Type | Constraints |
|---|---|---|
| id | TEXT | Primary key |
| batch_id | TEXT | FK to newspaper_batches |
| edition_code | TEXT | FK to newspaper_editions |
| publication_date | TEXT | YYYY-MM-DD |
| status | TEXT | JobStatus |
| output_dir | TEXT | Not null |
| page_count | INTEGER | Non-negative |
| completed_count | INTEGER | Non-negative |
| failed_count | INTEGER | Non-negative |
| original_bytes | INTEGER | Non-negative |
| final_bytes | INTEGER | Non-negative |
| warning | TEXT | Nullable |
| created_at | INTEGER | Not null |
| updated_at | INTEGER | Not null |
| completed_at | INTEGER | Nullable |

Unique key: `(edition_code, publication_date, output_dir)`.

### `newspaper_pages`

| Field | Type | Constraints |
|---|---|---|
| id | TEXT | Primary key |
| job_id | TEXT | FK to newspaper_jobs |
| page_number | TEXT | Not null |
| section_name | TEXT | Nullable |
| source_url | TEXT | HTTPS |
| original_path | TEXT | Nullable |
| optimized_path | TEXT | Nullable |
| status | TEXT | PageStatus |
| attempts | INTEGER | Non-negative |
| original_bytes | INTEGER | Nullable |
| final_bytes | INTEGER | Nullable |
| checksum | TEXT | Nullable |
| error | TEXT | Nullable |
| created_at | INTEGER | Not null |
| updated_at | INTEGER | Not null |

Unique key: `(job_id, page_number)`.

### `newspaper_events`

| Field | Type | Constraints |
|---|---|---|
| id | INTEGER | Autoincrement primary key |
| batch_id | TEXT | Nullable FK |
| job_id | TEXT | Nullable FK |
| event_type | TEXT | Not null |
| message | TEXT | Not null |
| payload_json | TEXT | Nullable valid JSON |
| created_at | INTEGER | Not null |

### `newspaper_settings`

| Field | Type | Constraints |
|---|---|---|
| key | TEXT | Primary key |
| value_json | TEXT | Valid JSON |
| updated_at | INTEGER | Not null |

## Out of Scope

- OS-1: Downloads while the LinkVault process is fully closed.
- OS-2: Windows Task Scheduler integration or a system tray.
- OS-3: OCR and article-text extraction.
- OS-4: PDF generation.
- OS-5: Automatic optimization of the existing archive.
- OS-6: Cloud synchronization.
- OS-7: Deleting downloaded editions from the library.
- OS-8: Circumventing publisher access restrictions.
- OS-9: Unrelated LinkedIn or Coursera redesign.
- OS-10: More than one concurrently active edition per batch.

## 10. Implementation Gates

1. **Phase 0:** Preserve current dirty UI work, validate this spec, restore or replace missing verification scripts, benchmark compression, and verify the edition catalog.
2. **Phase 1:** Add isolated newspaper schema and catalog/manifest core with tests.
3. **Phase 2:** Add durable page downloader, partial completion, cancellation, and restart recovery with tests.
4. **Phase 3:** Add app-open scheduling, delay, optimization, and preview generation with tests.
5. **Phase 4:** Add the compact downloader UI and browser-preview fixtures.
6. **Phase 5:** Add the compact library, local reader, and existing-archive registration.
7. **Phase 6:** Run full regression, live UAT, installer, and release-readiness checks.
