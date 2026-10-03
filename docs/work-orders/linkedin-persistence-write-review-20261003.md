# LinkedIn persistence write review, 2026-10-03

The v0.2.27 persistence gate stopped at `path_library.rs`: its lexical primitive count was 19 while baseline version 19 expected 11. The feature commit added membership/placement operations without updating the reviewed inventory. The review also found real writer violations, so changing counts alone was insufficient.

## Traced writes and repair

- `PathLibrary::ingest_expansion` and `add_course_to_path` already submitted their writes to `DatabaseWriter`, but a closure is not automatically a transaction. Both now use an explicit transaction; a late failure rolls back earlier membership, placement, and standalone changes.
- `CourseLayout::load` formerly inserted missing standalone placement rows. The workflow executor and real legacy command fallback called it using an `open_runtime` connection. It is now read-only. Planning requires `PathLibrary::course_layout`, which freezes a missing legacy placement through the existing shared writer before returning its layout.
- The artifact downloader formerly called `record_video_file_on_connection` using the download thread's runtime connection. It now requires `PathLibrary` and calls its writer-backed `record_video_file`. The raw helper is private to PathLibrary. Successful downloads and reused completed files both use this route.
- Expansion, folder-import placement, and fallback placement keep their existing first-writer placement policy. Import retains its existing explicit transaction; no job rows, schema, scheduler, or source data format change.
- The executor receives a clone of the existing writer through PathLibrary. Real saved-token, batch, and browser-token legacy paths pass that same writer owner through the existing helper chain. There is no optional production bypass.
- Periodic `linkedin_save_progress` previously waited synchronously for writer acknowledgement; `linkedin_list_catalog` previously queried SQLite synchronously. Both commands now move their database work into `spawn_blocking`, retaining IPC names, caller arguments, and result payloads.

## Inventory version 20

`path_library.rs` has 22 lexical primitives: the 19 scanned sites present in v0.2.27, two transaction starts, and one writer submission for fallback placement. The scanner includes embedded `cfg(test)` helper methods before the final test module and counts `.execute` writer submissions alongside SQL calls; these counts are not a semantic count of production SQL writes. The scanner's existing boundary behavior remains unchanged.

`placement.rs` has two INSERT sites: expansion/import placement and fallback standalone placement. Both production routes are writer-owned after the repair. The existing legacy helper/write inventory elsewhere stays unchanged. Matching this inventory alone does not prove writer ownership; the ownership traces and regression tests are required evidence.

The persistence structural gate additionally rejects direct runtime placement/video-index routes in download orchestration and requires PathLibrary dispatch.

Inventory version 21 records the separately reviewed newspaper reader repair: `reader_service.rs` retains its existing transaction and two INSERT primitives; the newly added mandatory `DatabaseWriter::execute` submission raises the lexical count from three to four. Its canonical page validation and read-mark/progress transaction now run together on the existing shared writer. No SQL site or writer owner was added.

## Focused regression coverage

- A trigger aborts a late standalone insertion after path and placement writes; failed expansion leaves all affected tables empty.
- A trigger aborts the last path update during membership reassignment; the original standalone row remains and no partial membership appears.
- A query-only runtime reader can load a missing layout without a write. The writer operation then freezes exactly one placement and records one completed writer request.
- A completed-video index operation works while the runtime reader is query-only and records one completed request on the shared writer.
- A source contract verifies both periodic player commands are asynchronous and execute their database operation inside `spawn_blocking`.

The repaired LinkedIn suite passed 227 tests with one ignored, including the periodic player command contract. The repaired persistence gate passed all 59 tests, including the four new persistence regressions. The canonical release persistence baseline also passed: 800 accepted and completed writes, zero failures, 1,450 ms contention time (limit 5,000 ms), 1 ms snapshot read (limit 250 ms), and a structurally redacted diagnostic sample. Its initial sandbox attempt failed to spawn Cargo with EPERM; the approved retry ran the same verifier successfully. Broader build, clippy, and native measurements are tracked by the parent integration task; this document does not promote structural tests into native UI evidence.
