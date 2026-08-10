# Newspaper Clippings V1: verification, performance, and release gates

**Status:** Approved

**Primary implementation phase:** Phase 6, with mandatory checkpoints in every
prior phase

**Related decisions:** All V1 decisions, especially D-004 through D-008,
D-015 through D-028, and D-032 through D-034

## 1. Purpose

This specification defines how Newspaper Clippings V1 is proven correct before
release. It maps product, persistence, filesystem, crop, reader, editor,
navigation, reset, accessibility, security, and performance requirements to
repeatable evidence.

A green unit-test suite alone is not sufficient. The feature combines SQLite,
managed files, large-image processing, a pointer-driven virtualized reader,
React 19 editor integration, autosave, native Windows DPI behavior, and
provider-reset semantics. Completion therefore requires four kinds of evidence:

1. **Deterministic automated tests** for contracts and failure paths.
2. **Structural verification** that prevents architectural regressions.
3. **Release-build measurements** for latency, memory, and bounded loading.
4. **Installed native Windows UAT** for behavior browser automation cannot
   faithfully prove, especially display scaling and Chinese IME.

No phase may weaken an existing gate, loosen a threshold without evidence, mark
failing coverage ignored, or substitute screenshots for executable tests.

## 2. Verification ownership

Recommended new scripts:

```text
apps/desktop/scripts/verify-newspaper-clippings.mjs
apps/desktop/scripts/verify-newspaper-clippings-browser.mjs
apps/desktop/scripts/verify-clipping-note-durability-structure.mjs
apps/desktop/scripts/verify-clipping-note-durability-browser.mjs
```

Recommended package scripts:

```json
{
  "verify:newspaper-clippings":
    "node --experimental-strip-types ./scripts/verify-newspaper-clippings.mjs",
  "verify:newspaper-clippings-browser":
    "node ./scripts/verify-newspaper-clippings-browser.mjs",
  "verify:clipping-note-durability-structure":
    "node ./scripts/verify-clipping-note-durability-structure.mjs",
  "verify:clipping-note-durability-browser":
    "node ./scripts/verify-clipping-note-durability-browser.mjs"
}
```

The repository-root package may expose forwarding aliases if that matches the
existing command pattern. Final release verification must invoke the new gates
or explicitly document why the existing release aggregator already does so.

### FR-VERIFY-001

`verify:newspaper-clippings` owns deterministic structural/data checks that do
not require a browser UI, including fixture validation and source scanning that
is inappropriate for ordinary unit tests.

### FR-VERIFY-002

`verify:newspaper-clippings-browser` owns Playwright/browser-harness behavior,
virtualization, keyboard, pointer, autosave UI, and route-state checks.

### FR-VERIFY-003

Rust unit/integration tests remain the authority for SQLite, filesystem,
media-protocol, image-pixel, recovery, concurrency, and command/service
contracts.

### FR-VERIFY-004

Native UAT remains a separately recorded completion gate. Browser automation
must not claim to prove Windows IME or physical display-scaling behavior.

## 3. Required command set

Every implementation PR runs the commands required by its phase. Final Phase 6
runs all commands below from a clean working tree.

### Frontend and repository gates

```powershell
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:persistence
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:visual
npm.cmd --prefix apps\desktop run verify:newspaper-performance
npm.cmd --prefix apps\desktop run verify:newspaper-performance-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
npm.cmd --prefix apps\desktop run verify:newspaper-clippings-browser
npm.cmd --prefix apps\desktop run verify:clipping-note-autosave
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-structure
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-browser
```

### Rust gates

```powershell
cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
```

### Final release gate

```powershell
npm.cmd --prefix apps\desktop run verify:release
```

If `verify:release` does not yet invoke both clipping gates, Phase 6 updates it
and its tests/documentation. It must not replace the explicit commands above in
phase PR evidence.

### FR-VERIFY-005

Evidence records exact command, working commit, operating system, exit code,
elapsed time, and relevant output summary. “Tests pass locally” without command
and commit is not acceptable phase evidence.

## 4. Requirement-to-evidence traceability

Each implementation PR must include a table mapping every implemented
requirement and acceptance criterion to one or more tests/evidence artifacts.

Template:

| Requirement/AC | Test or evidence | Layer | Result |
|---|---|---|---|
| `FR-CROP-005` | `normalized_rect_rejects_non_finite_values` | Rust unit | Pass |
| `AC-READER-009` | `reader_clipping_preserves_three_page_mount_bound` | Playwright | Pass |
| `AC-RESET-001` | `reset_preserves_clipping_rows_and_assets` | Rust integration | Pass |
| `FR-EDITOR-003` | Native IME case N-IME-04 | Installed UAT | Pass |

### FR-TRACE-001

No acceptance criterion may be marked complete solely by manual inspection when
it can be deterministically automated.

### FR-TRACE-002

Manual evidence identifies the automated limitation it covers, for example
Windows IME candidate-window behavior or cross-monitor DPI transition.

### FR-TRACE-003

A test name must describe behavior, not only an issue number or implementation
function.

## 5. Standard fixture sets

All fixtures are created in test-owned temporary directories and removed after
the test. No automated test reads or changes the user’s real `LinkVaultData` or
newspaper download folders.

### 5.1 Database fixture sizes

```text
SMALL  = 8 clippings
MEDIUM = 50 clippings
LARGE  = 500 clippings
```

Each set includes:

- Mixed English and Chinese titles/notes.
- Empty and non-empty notes.
- Long titles near the limit.
- Source-available and source-unavailable records.
- Ready and missing asset states.
- Multiple clippings from one page.
- Identical crop geometry with different clipping IDs.
- Deterministic created/updated timestamps with sort ties.
- Literal `%`, `_`, backslash/escape, apostrophe, punctuation, and date/page
  search values.

### 5.2 Reader fixture sizes

```text
SMALL  = 8 pages
MEDIUM = 50 pages
LARGE  = 500 pages
```

Each manifest uses generated page images with known dimensions and visible
coordinate markers. Only a bounded virtual range may mount regardless of
manifest size.

### 5.3 Image fixtures

Use the generated fixtures required by specification 03:

- Coordinate grid.
- Opaque JPEG.
- Alpha PNG.
- Existing WebP.
- Thin high-frequency text pattern.
- Edge-boundary dimensions.
- Corrupt/truncated file.
- Unsupported content under an image extension.
- Symlink/out-of-root source.
- Orientation metadata case.
- Mutating source.
- Oversized/decode-limit case.

### 5.4 Filesystem recovery fixtures

For one clipping ID, create isolated states representing every creation and
deletion crash point:

```text
staging .part only
complete staging without row
creating row + staging
creating row + canonical
creating row only
ready row + canonical
ready row + missing/corrupt canonical
ready row + unexpected symlink
ready row + orphan thumbnail
delete_pending row + canonical
delete_pending row + trash
deleted row + trash orphan
canonical orphan without row
quarantine candidate
```

### 5.5 Editor fixture

Use the common Markdown/Chinese fixture from specification 05 plus:

- Empty document.
- 2 MiB boundary document.
- Invalid raw HTML/MDX/image/table/code input.
- Safe and unsafe links.
- Rapid document switches.
- Two concurrent revision clients.

## 6. Persistence and migration test suite

Test prefix recommendation:

```text
clipping_persistence_
```

### Required tests

#### New database

- Initializes clipping table and indexes.
- Records current schema version.
- Creates no empty backup.
- Uses current connection policy and WAL behavior.

#### Upgrade database

- Creates verified backup before migration.
- Preserves representative rows from every provider.
- Preserves Newspaper edition catalog, jobs, pages, schedules, progress, and
  settings.
- Creates clipping schema with both `SET NULL` foreign keys.
- Writes schema version only after complete success.
- Is idempotent on repeated startup.

#### Failure and future version

- Backup failure blocks migration.
- Backup integrity failure blocks migration.
- Migration failure leaves old version and backup.
- Future schema is rejected before backup/change.
- Runtime open does not run schema work.

#### Write ownership

- Every clipping insert/update/state/delete uses `DatabaseWriter`.
- No image/filesystem/network work appears inside a writer closure.
- Reads continue under WAL while a writer is active.
- Concurrent revision updates produce one winner and one conflict.
- Writer shutdown resolves or explicitly rejects every accepted clipping write.

#### Reset/source deletion

- One source-page delete unlinks one/multiple clippings without changing notes.
- Job cascade unlinks source IDs without deleting clipping rows.
- Reset preserves clipping rows/assets/thumbnails.
- Reset preserves byte-identical note Markdown and checksums.
- Reset works with foreign keys enabled.
- Explicit unlink path protects legacy/test connections where foreign keys are
  disabled.
- `foreign_key_check`, `quick_check`, and representative reads pass afterward.

### AC-VERIFY-PERSIST-001

The clipping migration and reset suite passes from at least:

- A fresh database.
- The immediately previous supported schema.
- A populated schema-v3 clipping table backfilled to the legacy root with a
  verified backup and byte-identical note/asset metadata.
- A populated realistic test database with all providers represented.

### FTS migration and integrity

- Bundled SQLite reports `ENABLE_FTS5` before schema-v5 installation.
- A populated schema-v4 database receives a verified backup, FTS objects,
  triggers, rebuild, parity check, and only then `user_version = 5`.
- Insert, title/note update, delete, recovery-state change, reset/source unlink,
  and optimistic-conflict paths keep canonical rows and the index consistent.
- Forced trigger/rebuild failure rolls back without changing title/note bytes.
- Repair drops/rebuilds only derived FTS objects and restores representative
  English/Chinese results.
- Corrupt/missing index never causes clipping or note deletion.

## 7. Managed asset and recovery suite

Test prefix recommendation:

```text
clipping_asset_
clipping_recovery_
```

### Path and security tests

- Invalid/Unicode-lookalike operation IDs.
- Absolute paths.
- Parent components.
- Alternate separators.
- Symlinked root child.
- Symlinked canonical file.
- Directory in place of file.
- Empty file.
- Unsupported MIME/extension mismatch.
- Canonical path outside root.
- Thumbnail path outside root.
- Stale media version URL.
- Checksum mismatch.
- Missing/mismatched root marker and reused destination path.
- Offline removable/network root remains transient and is not recreated.
- Settings list reads registry rows without probing every root synchronously.
- Coalesced `Check again` returns connected/offline/marker-mismatch by root ID.
- Marker-verified reconnect updates only the requested root locator/key.
- Reconnect rejects empty/unmarked roots, another root's marker, duplicate
  locator ownership, reparse points, and database-write failure without moving
  or rewriting files.
- Open folder is rejected unless the current marker verifies.
- Search/note edit remain available with every canonical root offline.
- Error body/log redaction.

### Creation recovery tests

For every creation crash fixture, repeated recovery reaches the required
terminal state and remains idempotent.

### Delete recovery tests

For every delete crash fixture, repeated recovery completes confirmed deletion
without touching source media or another clipping.

### Orphan cleanup tests

- Grace period respected.
- Staging orphans and legacy-root canonical orphans move to quarantine before
  deletion. Visible snapshot edition/date trees are never recursively swept.
- Trash orphan cleanup.
- Exact-ID derived-thumbnail orphan cleanup without touching malformed,
  prefix-lookalike, symlinked, or another clipping's entries.
- Seven-day quarantine retention.
- Actual managed-directory iterator consumption is reported honestly for
  complete detached enumeration.
- Mutation attempts are bounded to 32 independently per category per launch.
- 500-entry and 5,000-entry Windows measurements record wall time and
  approximate/peak memory.
- No recursive scan outside managed root.
- Cleanup ownership is root-scoped; an ID in one root does not retain an
  orphan staging/trash entry in another root.
- Archive import and job deletion cannot descend into or remove
  `Newspaper snapshots`.

### Media protocol tests

- Current canonical request returns correct bytes/MIME/ETag/cache policy.
- Current thumbnail request returns correct cache bytes.
- Thumbnail bytes are read only after a positive 8 MiB metadata limit and are
  static decodable WebP within the 512×320 box.
- Empty, oversized-byte, oversized-dimension, malformed, and animated
  thumbnails fail safely without altering the clipping aggregate.
- Stale/malformed/missing/corrupt/symlink/escaped requests fail safely.
- Error responses use `no-store`.
- Absolute paths never appear in body/header.
- Only ready canonical assets are served.
- Root-unavailable requests do not transition ready rows to missing.

### AC-VERIFY-ASSET-001

No failure fixture modifies or removes a sentinel file outside the temporary
managed root.

## 8. Crop pipeline suite

Test prefix recommendation:

```text
clipping_crop_
```

The full requirements are in specification 03. Blocking categories:

### Geometry

- Every exact example.
- Edge/fraction/epsilon cases.
- Non-finite/negative/zero cases.
- Minimum-size boundaries.
- Checked overflow.
- 10,000 deterministic pseudo-random valid rectangles.
- Adjacent-edge behavior without independent-width rounding gaps.

### Source resolution

- Original preferred over optimized.
- Missing normal original falls back to optimized.
- Security-invalid original aborts.
- Stored versus decoded dimensions.
- Orientation behavior.
- Source changes during read.
- Unsupported/corrupt/oversized media.

### Pixel correctness

- Lossless decoded output equals source-region pixels.
- Alpha preserved.
- Output dimensions exact.
- Full-page crop exact.
- High-frequency pattern has no lossy artifacts.
- Reader tone and zoom do not affect backend request/result.

### Concurrency/idempotency

- One crop permit.
- Same operation ID coalesces/returns one row and asset.
- Different operation IDs queue without loss.
- No write transaction while waiting/processing.
- Shutdown reaches a recoverable state.

### AC-VERIFY-CROP-001

A `.webp` extension or successful decode is not enough to claim losslessness;
exact fixture-pixel equality is mandatory.

## 9. Reader/browser suite

Test prefix recommendation:

```text
reader_clipping_
```

Required cases from specification 04 include:

- Toolbar and `C` entry/exit.
- Editable-target and modifier exclusions.
- Escape hierarchy.
- Left/right navigation state matrix.
- Pan/click-zoom isolation.
- Four drag directions.
- Image-edge clamp.
- Resize/layout movement.
- Pointer cancel/lost capture/window blur.
- Scroll/zoom/page-control locking.
- Confirmation focus/actions.
- Duplicate submit guard.
- Success position restoration.
- Typed failure behavior.
- Stale refresh by page ID.
- Tone-independent payload.
- Reduced motion and live-region behavior.
- Capability hidden in production until Phase 4B.

### Structural reader budgets

These are blocking and do not require benchmark ratification:

```text
Mounted newspaper page media images <= 3
Hidden duplicate full-page images = 0
Full-page selection canvas copies = 0
Concurrent create invokes per one confirmation = 1
Stuck pointer captures/listeners/timers after cleanup = 0
```

### AC-VERIFY-READER-001

The existing Newspaper reader’s zoom, pan, progress save, page navigation,
virtualization, tones, close behavior, and 8/50/500-page performance suite remain
green with clipping capability enabled and disabled.

## 10. Clippings library/browser suite

Test prefix recommendation:

```text
clippings_library_
```

### Paging and virtualization

- 0/1/8/50/51/500 rows.
- Sparse page model.
- Page-aligned fetches.
- One near-future page prefetch maximum.
- Stable keys.
- Query-generation stale response rejection.
- Invalidation without scroll reset.
- Selection fallback after delete.
- Deep target outside search.

### Structural library budgets

```text
Page size = 50
Virtual row overscan = 4
Mounted list rows <= visible rows + 8 defensive rows
Full Markdown bodies fetched for list = 0
Canonical full images mounted in detail = 1
Canonical full images mounted in list = 0
Thumbnail ensure requests = visible rows only, coalesced per ID
Offline root probes = coalesced per root, not per visible row
Confident search page size = 50
Possible matches total <= 25
Fuzzy candidate/window set = bounded and measured before release
```

A browser fixture must expose instrumentation so these conditions are asserted,
not judged visually.

### Search/sort

- Literal wildcard/escape handling.
- FTS query operators and quotes remain literal user text.
- One- and two-character Title/Edition/Date/Page matching with Note excluded,
  exact helper copy, and no Note tag/snippet; three characters enables Note.
- Mixed English/Chinese exact, substring, and typo candidates.
- Exact title, title prefix, weighted Title/Note/Edition relevance, and stable
  updated-time/ID ties against a committed golden ranking fixture.
- Exact Date/Page matching with proof that neither field is fuzzed.
- Cumulative factual Title/Note/Edition/Date/Page match tags.
- Safe bounded highlighted Note snippet.
- Confident pages 0/1/50/51/500 and one Possible matches page capped at 25.
- Confident IDs excluded from Possible matches.
- Fuzzy query under four Unicode scalar values returns no fuzzy page.
- Two-megabyte note does not cause per-keystroke full-note fuzzy scanning.
- Search generation cancellation, cross-page dedupe, and invalidation restart.
- Relevance enforced while searching; ordinary sort modes and ties resume after
  clear.
- Clear search.
- Search takeover retains current editor state; result activation and return
  preserve dirty guard, query, loaded boundary, anchor, and focus.

### Source card

- Ready image.
- Missing asset.
- Source available/unavailable.
- Lazy image decode.
- Optional expanded viewer focus restoration.
- No editable image node or image toolbar action.

## 11. Editor and autosave suite

Test prefix recommendation:

```text
clipping_editor_
clipping_autosave_
```

### Package/adapter

- React 19 production build.
- Strict Mode remount.
- Approved subset round-trip.
- Unsupported input sanitization/rejection.
- Safe/unsafe links.
- No network requests.
- Lazy editor chunk.
- No stale document content/history/composition between IDs.

### Autosave

- Exact 800 ms debounce.
- Canonical 5-second maximum wait during continuous typing.
- Recovery checkpoint at 500 ms quiet / 2-second maximum wait.
- No Tauri write per keystroke.
- Title-only, note-only, combined changes.
- Edit during in-flight save.
- No-op update.
- Failure and Retry.
- Flush on every specified boundary.
- Navigation block and explicit discard.
- Window blur.
- Window X prevents close, flushes/checkpoints, and hides only on success.
- Tray Quit/application/updater exit flushes/checkpoints and exits only on
  success.
- Failed, uncheckpointed/stale-conflict, stale-token, missing-owner, and
  timed-out lifecycle requests remain open and preserve the draft; a
  canonical conflict exits only with the exact newest checkpoint durable.
- Canonical save clears only the matching acknowledged checkpoint atomically.
- Crash/restart recovery is revision-, writer-session-, and sequence-aware.
- Maximum-size validation.

### Revision conflicts

- Two clients at same revision.
- Keep my changes.
- Use saved version.
- Copy my draft.
- Repeated conflict.
- Local draft never lost.

### Native Chinese IME

Automated browser tests cannot close this gate. Installed UAT must cover:

```text
N-IME-01 type simple Chinese phrase through candidate selection
N-IME-02 compose across bold/paragraph context
N-IME-03 Enter selects candidate without unexpected save/navigation
N-IME-04 punctuation/backspace during composition
N-IME-05 undo/redo after committed composition
N-IME-06 autosave only after stable editor transaction
N-IME-07 switch clipping after successful flush
N-IME-08 failed flush preserves composed draft
N-IME-09 title input composition
N-IME-10 app blur and restore during a completed composition
```

Record Windows version, IME, keyboard layout, editor package/version, and result.

## 12. Navigation, deletion, and reset suite

Test prefix recommendation:

```text
clipping_navigation_
clipping_delete_
clipping_reset_
```

### Navigation

- Reader success toast → exact clipping.
- Detail outside current search.
- Detail → exact source job/page.
- Media-version-changed notice.
- Source missing/incomplete.
- Back target and focus restoration.
- Dirty guard.
- Rapid stale targets.

### Highlight

- Correct normalized geometry.
- Scroll into view.
- Three-second expiry.
- Early cancellation triggers.
- Reduced motion.
- Non-interactive behavior.

### Delete

- Ready/missing asset.
- Thumbnail present/absent.
- Revision conflict.
- Filesystem/database failures.
- Every crash point.
- Shared source unaffected.
- Selection/focus fallback.

### Reset

- Ready and missing rows.
- English/Chinese notes.
- Canonical bytes unchanged.
- Thumbnail bytes/path preserved or validly regenerable according to spec.
- Source IDs null.
- App open on clean/dirty/saving clipping.
- Existing edition catalog preservation.
- Existing reset counts and other provider isolation.

## 13. Accessibility verification

### Automated checks

Use existing tooling where available and deterministic assertions for:

- Accessible names.
- Button pressed/disabled/busy states.
- Dialog role and initial focus.
- Focus restoration.
- Keyboard reachability.
- Live-region update count/content.
- Non-color text/icon indicators.
- Reduced-motion class/state behavior.
- No nested interactive-control violations in virtual rows.

### Manual checks

- Keyboard-only complete workflow.
- Optional Windows screen-reader observation where available; it is not a
  blocking V1 gate.
- Focus visibility in light/dark and page tones.
- Contrast review for selection border/mask, badges, warnings, save/conflict
  states, and disabled controls.
- 200% display scaling without clipped critical controls.

### AC-VERIFY-A11Y-001

A pointer is not required to enter/cancel Clip mode, save a confirmed selection,
open a note, edit/format, open source, return, or delete. Drawing the rectangle
itself is pointer-based in V1; an alternative coordinate-entry crop UI is not
required, but all surrounding workflow and cancellation must remain accessible.

This limitation must be documented honestly rather than claiming complete
keyboard-only crop drawing.

## 14. Security and privacy verification

### Required assertions

- No frontend response contains source/canonical absolute paths.
- No log/diagnostic contains title/note content, image bytes, cookies,
  authorization headers, or raw provider payloads.
- IDs cannot escape managed roots.
- Symlinks are rejected.
- Media versions prevent stale cache/path reads.
- Canonical checksum mismatch is surfaced.
- Search uses bound parameters and escaped wildcard characters.
- Markdown/links cannot execute MDX, HTML script, `javascript:`, `data:`, file,
  or unknown schemes.
- Editor loads offline with no telemetry/cloud request.
- Reset/delete affects only intended provider/aggregate data.
- Test sentinels outside managed roots remain unchanged.

### Dependency review

Phase 4A records:

- Exact editor package and transitive dependency delta.
- License/notice requirements.
- Known security advisories at evaluation date from primary package sources.
- Whether content parsing permits raw HTML/MDX and how it is disabled.

Phase 6 reruns the dependency/security review against the locked version before
release.

## 15. Release-build performance measurement

Architecture guidance requires measured thresholds rather than arbitrary
claims. Phase 2 and Phase 4B collect baselines; Phase 6 ratifies thresholds or
blocks release when behavior is visibly/unacceptably regressed.

### 15.1 Crop baseline artifact

Path:

```text
docs/performance/newspaper-clippings-crop-windows-YYYY-MM-DD.json
```

Required fields:

```json
{
  "schemaVersion": 1,
  "commit": "<sha>",
  "build": "release",
  "machine": {
    "os": "Windows",
    "cpu": "<model>",
    "logicalCores": 0,
    "ramBytes": 0
  },
  "cases": [
    {
      "sourceFormat": "jpeg",
      "sourceWidth": 0,
      "sourceHeight": 0,
      "sourceBytes": 0,
      "cropWidth": 0,
      "cropHeight": 0,
      "outputBytes": 0,
      "queueWaitMs": 0,
      "readMs": 0,
      "decodeMs": 0,
      "cropMs": 0,
      "encodeMs": 0,
      "validateMs": 0,
      "filesystemMs": 0,
      "databaseMs": 0,
      "totalMs": 0,
      "workingSetDeltaBytes": 0
    }
  ],
  "maxConcurrentCropSections": 1,
  "sqliteBusyFailures": 0,
  "uiStallEvidence": "<reference>"
}
```

### 15.2 UI/library/editor baseline artifact

Path:

```text
docs/performance/newspaper-clippings-ui-windows-YYYY-MM-DD.json
```

Required measurements at 8/50/500 clippings:

- Initial list query duration.
- Time to first visible row/thumbnail.
- Mounted row count.
- Thumbnail request count.
- Deep detail open duration.
- Canonical image decode/load duration.
- Search query response and visible update.
- Editor lazy-chunk load.
- First editable readiness.
- Typing/IME main-thread long-task evidence.
- Autosave request duration.
- Checkpoint p50/p95/max latency and submitted/coalesced write counts.
- Close/quit handshake p50/p95/max for clean, dirty, in-flight, and checkpoint
  fallback states.
- SQLite WAL growth during 10 minutes of near-limit continuous typing and after
  the idle checkpoint interval.
- List scroll dropped-frame/long-task evidence where tooling permits.
- Process memory before/after detail and after closing.

### 15.3 Reader baseline regression

Existing Newspaper reader performance reports remain authoritative. Add clipping
mode cases proving:

- Mounted images remain ≤3.
- Entering selection does not decode/copy full pages.
- Pointer movement does not create sustained long tasks or unbounded React
  renders.
- Save image work occurs outside the WebView/main-thread path.

### 15.4 Ratified budgets

Phase 6 writes measured thresholds into this section or a linked approved
performance addendum. Until then, the following structural limits are already
binding:

```text
Crop concurrency = 1
Reader mounted page images <= 3
List page size = 50
List overscan = 4
Canonical full images mounted = 1
Canonical images in list = 0
Visible-only thumbnail requests
Offline root probes coalesced per root
Confident search page size = 50
Possible matches total <= 25
No per-keystroke full-note fuzzy scan
No image processing in database transactions
No per-keystroke IPC autosave
No editor eager load on unrelated routes
Zero unhandled SQLITE_BUSY failures
Zero path-security violations
```

A threshold change requires before/after release-build evidence and reviewer
approval. Development-mode timing alone is not accepted.

## 16. Visual verification

Add deterministic visual fixtures for:

- World Journal sidebar with Clippings inactive/active.
- Clippings empty state.
- Populated four-column default gallery and responsive column variants.
- Ranked search takeover with cumulative match tags.
- Confident-results tail followed by a separated Possible matches section.
- Snapshot locations in connected/offline/marker-mismatch/checking states.
- Long Chinese/English title and note excerpt.
- Source available/unavailable.
- Asset missing warning.
- Reader Clip toolbar inactive/active.
- Selection and confirmation in original and inverted tones.
- Saving/waiting/error.
- Editor clean/dirty/saving/failed/conflict.
- Delete dialog.
- Narrow list/detail layout.
- Light and dark themes.

Visual baselines verify layout and accidental regressions; they do not replace
behavior, accessibility, DPI, or pixel-crop tests.

## 17. Installed Windows UAT plan

Record final evidence at:

```text
docs/uat/newspaper-clippings-windows-YYYY-MM-DD.md
```

### Environment matrix

- Current supported Windows 11 build.
- Installed LinkVault release bundle, not only `npm run dev`.
- Clean database and migrated realistic database.
- Display scaling 100%, 125%, 150%, 200%.
- Light and dark application themes.
- Original, soft, dim, inverted reader tones.
- Normal and maximized windows.
- At least one multi-monitor transition when available.
- Supported Chinese IME and English keyboard.

### Scenario matrix

#### UAT-001: Fresh save

Save top-left and bottom-right regions at multiple zooms; verify visual alignment,
legibility, source-resolution dimensions, non-disruptive reader return, and Open
note.

#### UAT-002: Multiple saves

Save several regions in sequence; verify one-at-a-time processing, unique notes,
no duplicate submit, and reader position retention.

#### UAT-003: Note editing

Type English/Chinese, format all approved Markdown elements, autosave, switch,
restart, and verify exact content.

#### UAT-004: Failure/retry

Exercise stale media and a safe simulated asset/database failure; verify no
duplicate or silent data loss.

#### UAT-005: Source round trip

Open exact source, view transient highlight, Back to clipping, and verify focus
and list position.

#### UAT-006: Reset preservation

Reset World Journal with saved notes; restart and verify canonical images/notes
remain with source unavailable.

#### UAT-007: Delete

Delete one of multiple clippings from the same page; verify only target is gone
and source remains.

#### UAT-008: Missing asset

Use a controlled test copy to remove/corrupt one canonical asset; verify warning,
note safety, retry image check, and no silent recrop.

#### UAT-009: Keyboard/accessibility

Operate all non-drawing actions by keyboard, verify focus and announcements,
confirm no reader shortcut fires in editor fields.

#### UAT-010: Long/large data

Review 500-clipping fixture, search, scroll deeply, open/edit, and return without
unbounded row/image loading or obvious UI stalls.

#### UAT-011: Ranked and fuzzy retrieval

Search representative English/Chinese Title, Note, Edition, Date, and Page
values; verify explainable order, factual cumulative tags, safe snippets,
the one/two-character Note exclusion helper, exact-only Date/Page, lazy
confident pages, and no more than 25 visibly separated Possible matches. Open
a result with a pending draft and return to the same query/anchor without data
loss.

#### UAT-012: Snapshot location reconnect

With test-owned removable or moved snapshot data, verify Connected, Offline,
Check again, marker mismatch, and Reconnect states. Reconnect only the matching
marker-bound folder, confirm notes remain searchable while offline, and confirm
that no file is copied, merged, created, or deleted.

### UAT result format

For each case:

```text
ID
Build/commit
Environment
Preconditions
Steps
Expected
Actual
Pass/Fail
Evidence path
Issue/waiver if failed
```

A failed UAT case is not converted to pass by documenting it as a “known issue”
unless the product owner explicitly approves a release waiver with impact and
follow-up issue. Data-loss, path-security, reset-loss, crop-correctness, or note
loss failures are never waivable for V1.

## 18. Phase-specific gates

### Phase 1

Must run:

- Focused clipping persistence/asset/media tests.
- `verify:architecture`.
- `verify:persistence`.
- Rust fmt/clippy/test.
- Frontend production build.
- `verify:release`.

No reader/editor UI evidence is required because it is prohibited in Phase 1.

### Phase 2

Adds:

- Full crop/geometry/pixel/source/idempotency suite.
- Release crop baseline.
- Existing Newspaper performance gates.

### Phase 3

Adds:

- Reader clipping browser suite.
- Visual fixtures for reader states.
- Native DPI smoke at all four scales.
- Existing reader 8/50/500 regression suite.

### Phase 4A

Adds:

- Editor evaluation artifact.
- React 19/Strict Mode/Markdown/security/bundle tests.
- Synthetic composition coverage and visible Windows dev-harness user
  acceptance; native Tauri IME remains in Phase 4B.

### Phase 4B

Adds:

- Library/editor/autosave/conflict browser suite.
- 8/50/500 clipping performance baseline.
- Visual fixtures and native DPI/IME checks.
- Production enablement only after gates pass.

### Phase 4C

Adds:

- Schema-v6 recovery checkpoint migration/repository/service suite.
- Pure autosave/checkpoint controller and sequence/conflict matrix.
- Native close-X, tray Quit, application/updater exit, timeout, stale-token,
  writer-shutdown-order, and crash/restart tests.
- Durability ownership/line-budget structural gate.
- Browser integration plus installed Windows lifecycle/crash-recovery UAT.
- Release measurements for write counts, checkpoint/exit latency, memory, and
  SQLite/WAL growth.

### Phase 5

Adds:

- Navigation/highlight/delete/reset/recovery suite.
- Installed lifecycle smoke.

### Phase 6

Runs every gate, ratifies measured thresholds, completes installed UAT, reviews
security/licenses, and records rollback/readiness.

## 19. Release blockers

Release is blocked by any of:

- Canonical crop differs from specified source pixels.
- Crop depends on reader zoom/tone/display scale.
- Schema migration lacks verified backup or loses representative data.
- Source deletion/reset removes or alters a clipping/note/canonical asset.
- Managed path escape, symlink traversal, absolute-path disclosure, or unsafe
  media/Markdown link execution.
- Creation/deletion crash state cannot recover deterministically.
- Revision conflict silently loses a draft.
- A native close/exit path can destroy the editor or shut down the database
  writer before acknowledged durability.
- A stale session/sequence can overwrite or clear a newer recovery checkpoint.
- Recovery draft bytes enter canonical FTS, logs, diagnostics, or list excerpts.
- Chinese IME cannot compose reliably in title/editor.
- Reader exceeds its mounted-page bound.
- List loads canonical images/full Markdown for all rows.
- Editor eagerly loads on unrelated routes without approved exception.
- Existing architecture, persistence, UI, visual, Newspaper performance,
  browser, Rust, or release gate regresses.
- A required native UAT scenario fails without an allowed waiver.
- Implementation includes a V1 out-of-scope feature that has no approved
  specification.

## 20. Rollback verification

Every implementation PR describes rollback at its phase boundary.

### Database rollback

After a released schema migration, code rollback to a version that rejects the
newer schema is not a safe user-facing rollback. Release planning must instead:

- Preserve the verified pre-migration backup.
- Avoid destructive downgrade SQL.
- Roll forward with a fix when user data has been created under the new schema.
- Document how support restores from backup only with explicit user-data loss
  warning for clippings created after migration.

### Feature disable

A UI capability may be temporarily hidden only if:

- Existing clipping rows/assets remain readable/recoverable.
- Startup recovery and media/data integrity continue to run.
- Hiding does not strand unsaved editor state.
- The reason and re-enable plan are documented.

### Asset rollback

Do not delete managed roots during rollback. Unknown/newer asset versions are
preserved and reported, not rewritten by older code.

## 21. Known V1 limitations to document

Final release notes/help must state, without overstating capability:

- Crop drawing requires a pointer; surrounding actions are keyboard accessible.
- A forced process termination or power loss may lose normally no more than the
  500 ms quiet checkpoint window and no more than 2 seconds during continuous
  typing. This is bounded recovery, not a zero-loss hard-crash guarantee.
- Missing canonical assets are not silently recopied from potentially changed
  source pages.
- Deleted source editions are not heuristically relinked after redownload.
- Search is local ranked title/note/provenance retrieval with bounded typo
  suggestions; it is not OCR/full article text, semantic search, or fuzzy
  Date/Page matching.
- One clipping contains one image/note.
- OCR, AI summary, annotations, tags, export, sharing, and sync are not included.

## 22. Phase 6 exit gate and final release definition

Phase 6 is complete only when:

1. All implementation phases are merged with their own evidence.
2. Every command in section 3 passes on the release candidate commit.
3. Crop and UI release baselines are committed and ratified thresholds are
   approved.
4. Installed Windows UAT passes at 100/125/150/200% scaling.
5. Native Chinese IME tests pass with recorded environment.
6. Migration from realistic data and reset preservation are reverified on the
   release candidate.
7. Security, path, Markdown/link, dependency, and license review is complete.
8. Visual baselines are approved in light/dark and required reader tones.
9. Rollback/forward-fix procedure and known limitations are documented.
10. No release blocker remains open.
11. `verify:release` passes after all clipping gates are integrated.
12. The product owner explicitly marks the master PRD and Phase 6 status
    Complete.

Only then may the release version be bumped and tagged through the repository’s
normal release process. Version bumps and publishing are outside all earlier
implementation PRs.
