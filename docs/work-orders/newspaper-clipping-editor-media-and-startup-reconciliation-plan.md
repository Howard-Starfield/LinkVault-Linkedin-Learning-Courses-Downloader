# Newspaper clipping editor, media, and startup reconciliation plan

Status: Complete; automated verification and final native UAT passed
Date: 2026-08-10
Branch: `codex/newspaper-clippings-phase4c-durability`

## 1. Outcome

This work order addresses three user-reported defects without changing the
approved clipping storage, note durability, or search contracts:

1. task-list and other structured editor text must begin on the same visual row
   as its marker/control;
2. gallery thumbnails must remain crisp at the default four-column desktop
   layout, and crop provenance must prove that canonical bytes come from a
   database-registered local newspaper file rather than a remote page URL;
3. application startup must recover interrupted clipping operations and later
   reconcile managed folders without putting an eager recursive scan on the
   startup-critical path.

The changes are intentionally split by owner. `ClippingNoteEditor.tsx` remains
the headless Tiptap adapter, gallery loading remains in
`NewspaperClippingList.tsx`, native media stays in the clipping service/assets
modules, and startup scheduling is extracted from `lib.rs` rather than growing
the application bootstrap into a second recovery implementation.

## 2. Confirmed current behavior

### 2.1 Editor row displacement

Tiptap renders a task list as `<ul data-type="taskList">`; each child `<li>`
has `data-checked`, a checkbox `<label>`, and a content `<div>` containing one
or more paragraphs. The former stylesheet targeted
`li[data-type="taskItem"]`, which the production DOM does not emit. Therefore
none of the intended flex-row or checkbox rules applied, and the global editor
paragraph rule also gave the first nested paragraph a `.72em` top margin.

This is a block-spacing ownership defect. A negative offset on the checkbox or
the whole editor would only hide it for one font size. The task-item content
owner must normalize the first and last nested paragraph margins and define
spacing for any additional paragraphs.

The owner-level fix targets the rendered `taskList > li[data-checked]` contract,
then normalizes only the first/last nested paragraph margins. The ordinary
paragraph baseline remains unchanged.

### 2.2 Crop and thumbnail provenance

The canonical crop path is native and local:

- React sends a page ID, media version, and normalized rectangle, never a URL
  or filesystem path.
- SQLite resolves registered `original_path` and `optimized_path` values under
  the persisted newspaper job output directory.
- the backend rejects relative paths, symlinks/reparse points, non-files,
  out-of-root targets, changed file identities, unsupported formats, and stale
  media versions;
- crop bytes are read with `fs::read`, decoded locally, cropped in intrinsic
  pixels, and encoded as lossless WebP;
- `source_url` is not part of `CropSourceRecord` and is not available to the
  crop implementation.

The approved source priority remains: use the valid local original when it is
geometry-compatible, otherwise use the valid local optimized file. That keeps
maximum crop fidelity and does not imply a network fetch.

The quality defect is downstream. Derived clipping thumbnails are capped at
512x320, while a four-column card can exceed 512 device pixels on high-DPI or
large desktop windows. CSS then enlarges the cache image. The cache schema must
be bumped and the density target increased without upscaling crops whose
canonical asset is smaller than the cache target.

### 2.3 Startup recovery and folder work

LinkVault already performs two distinct operations:

- synchronous transactional recovery after database initialization resolves
  `creating` and `delete_pending` rows;
- deferred cleanup scans only managed staging/assets/trash/quarantine/cache
  categories, uses a 24-hour orphan grace period and seven-day quarantine, and
  caps mutations at 32 per category.

The transactional pass belongs on the correctness path and is expected to be
small. The folder cleanup is currently spawned immediately during setup. It is
off the UI thread, but immediate background disk enumeration can still contend
with database warmup, WebView creation, antivirus, and removable-drive access.

The safe startup policy is therefore:

```text
database migration and integrity checks
  -> synchronous database-driven transactional recovery
  -> construct and show the application
  -> quiet startup delay
  -> one background blocking worker
  -> managed-category cleanup with existing grace and mutation budgets
```

No startup task may recursively crawl an arbitrary download destination, infer
identity from similar filenames, import an asset without its database row, or
automatically recrop from a current online page. Unknown visible files do not
contain canonical note/revision/provenance state and cannot be safely imported.

## 3. Decisions and acceptance criteria

### 3.1 Editor layout

- The ProseMirror content root remains styled through Tiptap
  `editorProps.attributes`.
- Task-item first/last paragraph margins are owned by the task-item content
  container.
- A checkbox and its first text line overlap vertically and remain in one flex
  row at normal and narrow widths.
- Additional task-item paragraphs and nested lists remain readable.
- Normal paragraphs, headings, slash commands, selection toolbar, composition,
  and Markdown serialization remain unchanged.

### 3.2 Thumbnail quality and local crop proof

- Bump the clipping thumbnail cache schema so old 512-pixel entries are not
  reused.
- Generate at most 1024x640 lossless WebP thumbnail bytes.
- Never enlarge beyond the canonical clipping's decoded dimensions.
- Keep one thumbnail generation permit and lazy viewport-triggered generation.
- Preserve the canonical clipping byte-for-byte while generating a thumbnail.
- Add a deterministic test with a deliberately unusable remote `source_url`
  and distinguishable registered local pixels; the crop must succeed from the
  local file and record the expected local-source kind/checksum/region.
- Add structural proof that the crop projection has no remote URL field.

### 3.3 Startup reconciliation

- Synchronous transactional recovery remains after database initialization and
  before clipping state is exposed.
- Managed-folder cleanup starts only after a quiet delay and runs in
  `spawn_blocking`, never in the Tauri setup callback or async executor thread.
- Only one startup folder-reconciliation job is scheduled per process.
- Existing containment, marker, grace-period, quarantine, streaming
  enumeration, and per-category mutation budgets remain authoritative.
- Offline or marker-mismatched roots remain retryable and are never recreated,
  rebound, or marked corrupt by the scan.
- Startup scanning never follows symlinks/reparse points and never scans the
  visible edition/date tree for filename-based imports.
- Diagnostics contain counts/timing/error classes only, never user note bytes
  or absolute asset paths.

## 4. Implementation slices

### Slice A: editor block layout

1. Add task-content spacing rules to the production stylesheet.
2. Extend the browser matrix with task-item DOM and bounding-rectangle proof.
3. Run the editor static/browser gates and production build.

### Slice B: thumbnail density and provenance

1. Bump the clipping thumbnail cache schema and size constants.
2. Clamp generation to canonical dimensions before resizing.
3. Extend Rust tests for large-cache output, no-upscale output, cache-version
   replacement, canonical-byte preservation, and local-only crop provenance.
4. Extend structural/browser gates to cover the high-DPI four-column contract.

### Slice C: startup scheduling

1. Extract startup clipping reconciliation scheduling into a bounded newspaper
   module.
2. Keep transactional recovery synchronous.
3. Delay the existing managed-folder cleanup, then run it on one blocking
   worker.
4. Add source-level ordering/ownership gates and focused Rust tests for the
   policy constants and one-shot scheduling boundary.

### Slice D: integration and audit

1. Run focused editor, gallery, crop, recovery, persistence, and architecture
   gates.
2. Run TypeScript/build, Rust formatting/clippy/tests, and diff/ignore audits.
3. Re-launch the native application on an isolated port and request one final
   UAT covering the three user-visible outcomes.

## 5. Performance and conflict gates

- No new runtime dependency.
- No synchronous filesystem or database work in a Tiptap transaction.
- No remote client is introduced into the crop service.
- Thumbnail work remains lazy and single-concurrency; 8/50/500 gallery browser
  profiles remain bounded.
- Main-window first paint is not blocked by managed-folder enumeration.
- Existing note checkpoint/canonical-save ordering, FTS behavior, close-X/tray
  Quit protocol, root reconnect marker checks, and World Journal reset
  preservation remain unchanged.
- `ClippingNoteEditor.tsx` stays below its 500-line gate. New startup behavior
  belongs in a separate bounded module rather than `lib.rs` or the already
  large clipping service.

## 6. Required proof

Focused first:

```powershell
npm.cmd --prefix apps\desktop run verify:clipping-note-editor
npm.cmd --prefix apps\desktop run verify:newspaper-clipping-library
npm.cmd --prefix apps\desktop run verify:newspaper-clipping-library-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml clipping_crop -- --test-threads=1
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml clipping_startup -- --test-threads=1
```

Then integration:

```powershell
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:persistence
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-structure
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-browser
cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml -- --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
git diff --check
```

The prior dev-server browser timeout waiting for a transient `Saving...` state
is not accepted as a product result; final browser proof must use an owned
built preview and preserve the exact failing output if a gate fails.

## 7. Final native UAT

Only after all automated gates pass:

1. Create a to-do item and confirm checkbox plus first text line share one row.
2. View four clipping thumbnails on a high-DPI/default-width window and confirm
   their text remains sharp without placeholder flashing.
3. Crop a known local downloaded page, temporarily disconnect the network, and
   confirm the clipping still saves and reopens.
4. Restart LinkVault and confirm startup remains responsive; an interrupted
   staged clipping in a disposable fixture is recovered without scanning or
   modifying unrelated download folders.

## 8. Implementation and verification record

- Production and evaluation editor CSS now target Tiptap's actual rendered
  task-list DOM. A built-preview browser test creates a to-do through the slash
  menu and checks computed display, paragraph margin, and checkbox/text
  vertical overlap.
- Clipping thumbnail cache schema `v2` uses a 1024x640 ceiling. Generation uses
  `DynamicImage::thumbnail` for an aspect-preserving fit and clamps target
  dimensions to the canonical crop, preventing small-image upscaling.
- The clipping media protocol now validates clipping cache schema `2`
  independently from the existing newspaper-page thumbnail schema `1`.
- A deterministic crop test stores an unusable loopback HTTP URL beside a
  registered local file and byte-compares the saved lossless crop with the
  expected local pixel region. The crop projection structurally excludes the
  URL field.
- Startup composition now performs transactional row recovery synchronously,
  waits five seconds, then schedules one managed-folder reconciliation on a
  blocking worker. Existing containment, grace, quarantine, streaming, and
  mutation-budget rules are unchanged.
- Final automated evidence: editor matrix 17/17; clipping library browser
  matrix passed including 8/50/500 profiles; crop suite 37/37; persistence
  44/44; full debug and release Rust suites 559 passed with four documented
  pre-existing ignores; release aggregator passed.
- Native UAT for editor alignment, thumbnails, local crop behavior, and startup
  responsiveness passed on 2026-08-10.

## 9. Readable clipping folder follow-up

New clippings use this collision-safe visible path:

```text
Newspaper snapshots/<edition>/<date>/Page <page> - <full UUID>/clipping-v1.webp
```

The hierarchy already supplies edition and date, so they are not duplicated in
the leaf. Keeping the full UUID avoids same-page/same-time collisions without a
database lookup or retry race. The page label is sanitized and bounded. Existing
UUID-only folders remain valid and are not renamed, because bulk filesystem and
SQLite path migration would add failure modes without improving existing note
identity or search.

Automated follow-up evidence: 17 asset path/security tests, 38 crop tests, seven
media-protocol tests, 44 persistence tests, and the full 564-test Rust suite
passed (560 executed; four documented pre-existing ignores). The exact
same-page/same-timestamp collision regression passed with both canonical files
intact. Final native UAT passed on 2026-08-10: a newly saved clipping used the
readable edition/date/page-plus-UUID hierarchy and reopened successfully.
