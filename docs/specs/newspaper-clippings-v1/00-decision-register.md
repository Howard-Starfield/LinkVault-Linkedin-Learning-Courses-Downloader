# Newspaper Clippings V1 decision register

**Status:** Approved with the V1 specification set

**Last updated:** 2026-08-10

This register is the authoritative record for choices that materially constrain
Newspaper Clippings V1. A coding agent may not replace an approved decision
with a local implementation preference. Any change requires a new entry that
explicitly supersedes the earlier decision and identifies affected documents,
migrations, tests, and rollback impact.

## Status vocabulary

- **Approved:** Binding for V1.
- **Proposed:** Recommended, but implementation is blocked until approved.
- **Deferred:** Deliberately postponed beyond the current phase or V1.
- **Rejected:** Considered and intentionally not selected.
- **Superseded:** Replaced by a later approved decision.

## Summary

| ID | Decision | Status |
|---|---|---|
| D-001 | Feature and sidebar label is `Clippings`. | Approved |
| D-002 | Reader action is `Clip` with shortcut `C`. | Approved |
| D-003 | V1 selection is one rectangle on one page. | Approved |
| D-004 | Canonical capture is a native source-image crop. | Approved |
| D-005 | Frontend sends normalized coordinates; backend persists source pixels. | Approved |
| D-006 | Expected page media version is required and stale media is rejected. | Approved |
| D-007 | Source priority is retained original, then current optimized image. | Approved |
| D-008 | Canonical output is lossless WebP with no resize. | Approved |
| D-009 | Canonical assets are application-managed durable data. | Superseded by D-032 |
| D-010 | One clipping owns one note in V1. | Approved |
| D-011 | SQLite Markdown is the note source of truth. | Approved |
| D-012 | Source image is a fixed card outside the editable document. | Approved |
| D-013 | Saving keeps the reader open and offers `Open note`. | Approved |
| D-014 | Default title derives from edition, date, and page. | Approved |
| D-015 | Clippings use a two-pane paged and virtualized library. | Approved |
| D-016 | Source deletion and World Journal reset preserve clippings. | Approved |
| D-017 | Source provenance is denormalized and foreign keys use `SET NULL`. | Approved |
| D-018 | Note updates use optimistic revisions. | Approved |
| D-019 | Search uses ranked local FTS with bounded fuzzy suggestions. | Approved |
| D-020 | Derived thumbnails are regenerable cache data. | Approved |
| D-021 | Clipping media is served by the protected newspaper media protocol. | Approved |
| D-022 | Crop work is bounded and performed outside database transactions. | Approved |
| D-023 | Editor integration is hidden behind an internal Markdown adapter. | Approved |
| D-024 | Exact WYSIWYG package is selected by a gated compatibility spike. | Approved |
| D-025 | Plain Markdown subset excludes executable MDX and arbitrary HTML. | Approved |
| D-026 | Autosave debounce is 800 ms with explicit flush boundaries. | Approved |
| D-027 | Clipping deletion is explicit and removes note plus managed asset. | Approved |
| D-028 | Missing source and missing asset are separate UI states. | Approved |
| D-029 | OCR, AI, annotations, multiple attachments, tags, and sync are deferred. | Deferred |
| D-030 | Canonical screenshots are rejected for V1. | Rejected |
| D-031 | Deferred cleanup fully enumerates managed categories with bounded mutations. | Approved |
| D-032 | New canonical assets live in a registered snapshot root under the source download destination. | Approved |
| D-033 | Settings manages registered snapshot locations and marker-verified reconnection. | Approved |
| D-034 | Native recovery checkpoints and cooperative exit protect clipping-note drafts. | Approved |

---

## D-001: Feature and sidebar label

**Status:** Approved

**Decision:** The World Journal navigation adds a third child named
**Clippings**. The page heading and empty-state language use the same term.

**Rationale:** “Clipping” is the established mental model for preserving a
newspaper excerpt together with notes. “Snapshot” emphasizes capture mechanics
but not the resulting durable reading artifact. “Saved sections” is less
specific and conflicts with newspaper section terminology.

**Rejected alternatives:** `Snapshots`, `Saved sections`, `Reading notes`,
`Scrapbook`.

**Affected specifications:** 01, 05, 06.

## D-002: Reader action and keyboard shortcut

**Status:** Approved

**Decision:** The reader toolbar action is **Clip**, represented by the existing
Lucide scissors icon. Pressing unmodified `C` enters or exits clipping mode when
the reader canvas owns keyboard context and no editable element is focused.

**Rationale:** The label is compact enough for the reader header and matches the
feature name. An unmodified mnemonic shortcut is discoverable and does not
conflict with existing left/right page navigation.

**Constraints:**

- `C` is ignored in inputs, selects, textareas, contenteditable regions, and the
  future clipping note editor.
- `Ctrl/Cmd+C` remains copy and never toggles clipping mode.
- Repeated keydown events are ignored.
- `Escape` has state-dependent behavior defined in specification 04.

**Affected specifications:** 01, 04.

## D-003: Selection scope

**Status:** Approved

**Decision:** A V1 clipping contains exactly one axis-aligned rectangular region
from exactly one newspaper page.

**Rationale:** This is deterministic, maps directly to source pixels, works with
the existing virtualized reader, and avoids annotation-layer or multi-page data
models before the basic workflow is proven.

**Deferred alternatives:** Multiple rectangles, lasso selection, article-column
detection, cross-page clipping, stitched captures.

**Affected specifications:** 01, 03, 04.

## D-004: Canonical capture mechanism

**Status:** Approved

**Decision:** Rust crops registered source media. The WebView never supplies
screenshot bytes for canonical clipping creation.

**Rationale:** Source cropping preserves available resolution and excludes CSS
page tone, reader zoom, viewport clipping, display scaling, browser
rasterization, overlays, and accidental UI chrome.

**Consequences:** The backend must resolve and validate the page, decode image
bytes, convert normalized coordinates to source pixels, encode the clipping,
and maintain a managed-file lifecycle.

**Affected specifications:** ADR-002, 02, 03, 04.

## D-005: Coordinate contract

**Status:** Approved

**Decision:** React sends finite normalized coordinates measured against the
rendered `<img>` rectangle. Rust validates those values against decoded source
dimensions, applies the specified floor/ceil conversion, and persists integer
source-pixel coordinates.

**Rationale:** Normalized request coordinates remain independent of reader zoom,
window size, device-pixel ratio, and CSS layout. Persisted pixels are
deterministic and can reconstruct the region without recreating browser
geometry.

**Affected specifications:** 02, 03, 04.

## D-006: Media-version consistency

**Status:** Approved

**Decision:** Every create request includes the page media version displayed by
the reader. The backend rejects a mismatch with `SOURCE_MEDIA_STALE` and checks
again before committing the clipping record.

**Rationale:** Page optimization can replace display media while the reader is
open. A silent crop against a different version would make selection and saved
pixels disagree.

**User behavior:** The selection remains visible and the reader offers a retry
after refreshing the page manifest. It does not silently rebind the request.

**Affected specifications:** 03, 04.

## D-007: Source priority

**Status:** Approved

**Decision:** The crop service uses the first valid candidate in this order:

1. Retained original page image.
2. Current optimized page image.
3. Typed `SOURCE_MEDIA_UNAVAILABLE` failure.

**Rationale:** A retained original is the highest-fidelity provenance source.
When originals were intentionally removed after optimization, the optimized
page is still the authoritative available source and retains page dimensions.

**Constraint:** The chosen source kind, MIME type, checksum when available, and
media version are recorded as provenance snapshots. Raw source paths are not
persisted in the clipping row or returned to React.

**Affected specifications:** 02, 03.

## D-008: Canonical image format

**Status:** Approved

**Decision:** Canonical clipping output is lossless WebP, with no resize and no
second lossy quality pass.

**Rationale:** Newspaper text has hard edges and small glyphs that reveal lossy
artifacts. The repository already includes WebP encode/decode support, and
lossless WebP provides a compact canonical file without binding the result to
the source’s original lossy quality.

**Constraints:**

- Output dimensions exactly equal the validated source-pixel rectangle.
- The encoder must use an actual lossless mode; a quality value alone is not
  accepted as evidence of losslessness.
- Encoded bytes are decoded and dimensions verified before promotion.
- SHA-256 is calculated over final canonical bytes.

**Rejected alternatives:** PNG as the default, lossy WebP, JPEG, screenshot
PNG. PNG may be reconsidered only if the gated encoder test proves lossless WebP
unavailable or unreliable in the supported Rust build.

**Affected specifications:** 02, 03, 07.

## D-009: Asset ownership and location

**Status:** Superseded by D-032

**Decision:** Canonical clipping assets live under an application-managed root
beneath resolved `LinkVaultData/newspaper-clippings`. They do not live under the
user-selected edition output directory.

**Rationale:** A clipping is user-created durable data. Downloaded editions are
replaceable source data and may be moved, deleted, re-registered, or reset.

**Constraint:** SQLite stores a backend-generated relative path only. React
never chooses or receives the absolute canonical path.

**Affected specifications:** ADR-002, 02, 06.

## D-032: Download-destination snapshot roots

**Status:** Approved

**Decision:** A new clipping uses the persisted `newspaper_batches.destination`
of its source job. Its canonical image is stored beneath:

```text
<destination>/Newspaper snapshots/<sanitized edition name - code>/<publication-date>/Page <page> - <clipping-id>/clipping-v1.webp
```

The readable leaf repeats the page label but retains the full clipping UUID,
so multiple selections from the same edition, date, and page cannot collide.
Edition and date are not repeated in the leaf because the parent hierarchy is
already authoritative and Windows path-length headroom is finite. Existing
UUID-only snapshot leaves remain valid and are not renamed or migrated.

SQLite stores `asset_root_id` and a root-relative asset path. The backend owns
a root registry and a marker under the reserved `.linkvault` subtree. Staging,
trash, and quarantine are on that same volume. Thumbnails remain regenerable
cache data beneath `LinkVaultData`.

Existing schema-v3 rows are backfilled to a read-only `legacy_managed` root and
continue to resolve `LinkVaultData/newspaper-clippings`; migration does not move
their bytes. New creation accepts only `download_snapshot` roots.

An unavailable registered root is a transient storage state. Startup, media,
and cleanup do not recreate it or mark all of its rows missing. A reused path or
drive letter must present the matching marker. Archive import/repair and source
reset must exclude `Newspaper snapshots`. V1 does not scan for or automatically
rebind a moved root; a future reconnect flow must require the matching marker.

**Rationale:** The user wants crops visible beside the newspaper collection,
while the registry/marker boundary preserves deterministic ownership across
multiple destinations, offline drives, source deletion, and path reuse.

**Supersedes:** D-009 location only. Application ownership, backend-derived
paths, explicit deletion, and durable note semantics remain binding.

**Affected specifications:** ADR-002, README, 02, 06, 07, 08.

## D-033: Snapshot location management and reconnection

**Status:** Approved

**Decision:** Newspaper Settings shows the registered snapshot locations that
were created automatically from persisted newspaper download destinations. It
does not provide an arbitrary global snapshot-folder override.

Each location exposes a backend-derived display path. A newly opened Settings
view may briefly show `checking`; the verified outcome is `connected`,
`offline`, or `marker_mismatch`. **Check again** retries the registered
location. **Reconnect…** opens a backend-owned native folder selection flow for
an offline or mismatched root and updates the locator only after the selected
`Newspaper snapshots` directory presents the matching root marker. **Open
folder** is available only after current marker verification.

Reconnect never copies, merges, scans for, renames, or creates snapshot data.
It rejects an empty/unmarked directory, a marker for another root, a location
already registered to another root, symlinks/reparse points, and paths outside
the selected marker-bound root. Notes and search remain available while a root
is offline.

**Rationale:** “Sync again” would imply data transfer or filesystem indexing.
The actual operation restores the trusted locator for durable data that has
moved, while preserving the same-download-destination ownership model and
preventing drive-letter/path reuse from rebinding unrelated files.

**Affected specifications:** 02, 05, 06, 07, 08.

## D-010: Clipping-to-note cardinality

**Status:** Approved

**Decision:** Saving creates one clipping aggregate containing one canonical
image, one title, and one Markdown note. A note cannot own multiple clipping
images in V1.

**Rationale:** This creates a complete workflow with simple ownership,
delete/recovery semantics, and a clear two-pane library. Multiple attachments
would require ordering, reassignment, partial deletion, and generalized note
ownership decisions.

**Affected specifications:** 02, 05.

## D-011: Note source of truth

**Status:** Approved

**Decision:** SQLite stores `title` and `note_markdown`. Markdown is canonical;
editor-specific document JSON is not persisted.

**Rationale:** SQLite already provides migration, backup, serialization,
search, and local durability. Markdown remains portable and permits later
export without coupling data to a WYSIWYG package.

**Deferred alternative:** A per-clipping `note.md` bundle. It may be added as an
export format, but not as a second live source of truth.

**Affected specifications:** 02, 05.

## D-012: Source-card placement

**Status:** Approved

**Decision:** The canonical clipping image and provenance render as a fixed,
read-only document header above the title and editor. It may share one visual
writing surface with the note, but the image is not represented as a movable
or deletable node inside Markdown.

**Rationale:** The clipping is the evidence that gives the note meaning. Keeping
it outside the editor prevents accidental deletion, keeps provenance
structured, and allows the editor package to change independently.

**Affected specifications:** 01, 05.

## D-013: Post-save behavior

**Status:** Approved

**Decision:** A successful save returns to reader browse mode, displays a
success toast with edition/date/page provenance, and offers **Open note**. The
user is not automatically navigated away.

**Rationale:** Capturing should be a low-friction reading action. Forced
navigation would interrupt users who save several sections from one edition.

**Affected specifications:** 01, 04, 06.

## D-014: Default title

**Status:** Approved

**Decision:** New clipping titles use:

```text
<Edition name> · <YYYY-MM-DD> · <Page number>
```

Example:

```text
New York · 2026-08-07 · A06
```

**Rationale:** The value is deterministic, available without OCR, meaningful in
lists, and safe when an article headline is not fully included in the crop.

**Constraints:** The title is user-editable, trimmed, 1–200 Unicode scalar
values, and never generated by AI in V1.

**Affected specifications:** 02, 05.

## D-015: Clippings library layout and loading

**Status:** Approved

**Decision:** The Clippings view uses a responsive, virtualized thumbnail
gallery. Opening a thumbnail navigates to a separate full-page note document;
the gallery search is not repeated on that detail page. The default sort is
most recently updated.

**Rationale:** Four visual thumbnails at the default desktop width make saved
clippings faster to scan, while a separate document removes split-pane limits
from long-form note editing.

**Constraints:** Page size is 50, row overscan is 2, thumbnails are requested
only for visible rows, columns respond from 1 through 6 with 4 at the default
desktop width, and fixture gates cover 8, 50, and 500 clippings.

**Affected specifications:** 05, 07.

## D-016: Source deletion and reset preservation

**Status:** Approved

**Decision:** Deleting a source edition, clearing newspaper jobs/pages, or
using Reset World Journal preserves clipping rows, notes, canonical assets, and
derived clipping thumbnails.

**Rationale:** Clippings are durable user-created data. Reset is intended to
repair or clear replaceable provider downloads and must not destroy notes.

**Constraint:** Reset copy must state this preservation explicitly, and
persistence tests must prove it.

**Affected specifications:** 02, 06, 07.

## D-017: Provenance and nullable source links

**Status:** Approved

**Decision:** The clipping stores nullable source job/page foreign keys using
`ON DELETE SET NULL` and also stores immutable snapshots of edition code/name,
publication date, page number, media version, source dimensions, selected
source kind, source MIME type, and source-pixel crop rectangle.

**Rationale:** Foreign keys enable exact navigation while source data exists;
snapshots preserve context after it is removed.

**Affected specifications:** 02, 06.

## D-018: Optimistic note revisions

**Status:** Approved

**Decision:** Every mutable clipping update includes `expectedRevision`. The
repository updates only when it matches and increments revision on success.
Zero changed rows return `CLIPPING_REVISION_CONFLICT`.

**Rationale:** Multiple windows, stale detail loads, rapid navigation, or future
companion views must not silently lose user text.

**Affected specifications:** 02, 05.

## D-019: Search implementation

**Status:** Approved

**Decision:** V1 provides local relevance-ranked search over title, Markdown,
edition name/code, date, and page number. Schema v5 adds a rebuildable SQLite
FTS5 trigram index for title, note, and edition candidate retrieval. Exact and
prefix title matches are ranked ahead of weighted FTS relevance; date and page
matches are literal only. Updated time and clipping ID are deterministic final
ties.

Confident results lazy-load in pages of 50. After every confident result is
exhausted, the UI may request one separately labelled **Possible matches**
section capped at 25 unique rows. Fuzzy matching applies only to Title, Note,
and Edition, requires at least four Unicode scalar values, and operates only on
a bounded FTS candidate/window set. It never scans every full note per
keystroke. Date and Page are never fuzzed.

**Rationale:** The user needs fast keyword retrieval independent of the nested
snapshot folders, explainable field tags, useful typo tolerance, and stable
ranking. A derived local index provides these without making filesystem layout
or the index itself authoritative.

**Constraints:** Search input is normalized, trimmed, capped at 200 Unicode
scalar values, bound as data, and treated literally rather than as FTS syntax.
Title, Edition, Date, and Page may match from one Unicode scalar value. Note
body matching begins at three Unicode scalar values; shorter queries do not
scan note bodies and the UI explains the limit. Results return factual
cumulative match fields: `title`, `note`, `edition`, `date`, and `page`. No
confidence percentage is shown. While a query is active, the visible sort is
`Relevance`; ordinary list sort resumes after clearing it. The FTS index is
derived, transactionally synchronized, integrity-checked, and rebuildable
without changing title or note source data.

**Affected specifications:** 02, 05, 07.

## D-020: Derived thumbnail lifecycle

**Status:** Approved

**Decision:** List thumbnails are regenerable cache files derived from the
canonical clipping, generated on demand for visible rows. They preserve aspect
ratio, do not upscale, and fit within a 1024×640 pixel box.

**Rationale:** Loading full-resolution canonical crops in every visible row can
create avoidable decode and memory cost. Derived thumbnails can be safely
removed and rebuilt.

**Constraints:** Thumbnail cache schema version and canonical asset version are
part of the URL/version key. Thumbnail absence does not make a clipping
unavailable. The 1024×640 cache is schema version 2 so pre-density cache files
cannot be mistaken for current output.

**Affected specifications:** 02, 05, 07.

## D-021: Media protocol

**Status:** Approved

**Decision:** Canonical clipping and thumbnail bytes are served through new
versioned variants of the existing `newspaper-media` protocol.

**Rationale:** The protocol already keeps page and thumbnail filesystem paths
out of React and centralizes MIME, version, symlink, and error handling.

**Conceptual routes:**

```text
newspaper-media://clipping/<clipping-id>?v=<asset-version>
newspaper-media://clipping-thumbnail/<clipping-id>?v=<asset-version>-<cache-schema-version>
```

**Affected specifications:** 02, 05.

## D-022: Crop concurrency and transaction boundary

**Status:** Approved

**Decision:** Full-page crop work runs in a bounded blocking executor with one
concurrent crop permit in V1. No database transaction spans file read, image
decode, crop, encode, checksum, validation, or filesystem promotion.

**Rationale:** Newspaper pages are large and the current persistence contract
forbids holding writes across image work. One permit gives predictable memory
usage until release measurements justify a different limit.

**Affected specifications:** 02, 03, 07.

## D-023: Internal editor adapter

**Status:** Approved

**Decision:** Production components depend on a LinkVault-owned
`ClippingNoteEditor` interface, not directly on a third-party editor package.

**Rationale:** The persistent format is Markdown and the package must remain
replaceable. The adapter localizes plugin configuration, event semantics,
accessibility fixes, and Markdown normalization.

**Affected specifications:** 05.

## D-024: WYSIWYG package selection

**Status:** Approved — 2026-08-09

**Decision:** Use `@tiptap/core`, `@tiptap/react`, `@tiptap/starter-kit`,
`@tiptap/markdown`, `@tiptap/suggestion`, and `@tiptap/extension-list`, each
pinned at `3.29.2`, behind the LinkVault-owned `ClippingNoteEditor` Markdown
adapter. The suggestion utility is limited to a local, Markdown-safe
slash-command menu; the list extension enables Markdown task items. Persist
plain Markdown only; never persist Tiptap/ProseMirror JSON.

**Required evidence:**

- React 19 and Strict Mode operation.
- Synthetic composition coverage plus a visible Windows dev-harness user smoke.
- Plain Markdown round trips for the approved subset.
- Undo/redo across controlled parent updates; production autosave integration
  remains a Phase 4B gate.
- Keyboard toolbar, labels, focus order, and visible focus behavior.
- Offline operation with no remote dependency.
- Dark/light theme integration.
- Production build and bundle impact.
- License compatibility and third-party notice requirement.
- No executable MDX or arbitrary raw HTML requirement.

**Approval result:** Tiptap is approved from the committed Phase 4A comparison
in `docs/evaluations/newspaper-clipping-editor-2026-08-09.md`. MDXEditor 4.2.0
was rejected because raw executable-style Markdown persisted and its evaluated
dependency tree had unresolved audit findings. Tiptap passed the shared React
19, Strict Mode, Markdown-safety, offline, theme, bundle, license, audit, and
browser matrix; the product owner then exercised the visible Windows
evaluation harness and accepted its editing behavior.

Native Tauri Chinese IME validation moves to the Phase 4B exit gate, where the
real production autosave and document-switch owners exist. Screen-reader UAT
is not a Phase 4A, Phase 4B, or release blocker; keyboard semantics and labels
remain required and automated.

**Known limitations:** Phase 4A does not production-enable the editor, add
autosave, or prove Tauri WebView composition. Phase 4B must retain the adapter
boundary, lazy-load the editor, and run native IME cases against the integrated
desktop flow.

**Affected specifications:** 05, 07, 08.

## D-025: Markdown subset and executable content

**Status:** Approved

**Decision:** V1 supports paragraphs, headings 1–4, bold, italic,
strikethrough, unordered/ordered/task lists, blockquotes, links, horizontal
rules, and line breaks.
Executable MDX, JSX, arbitrary raw HTML, embedded scripts, remote iframes,
editor-inserted images, tables, and code blocks are excluded.

**Rationale:** This covers ordinary reading notes while keeping rendering,
security, round-trip tests, and toolbar scope bounded.

**Affected specifications:** 05.

## D-026: Autosave policy

**Status:** Approved

**Decision:** Note changes autosave 800 ms after the last edit. Dirty state is
flushed before clipping switch, route change, editor unmount, application blur,
and a cooperative native close request.

**Rationale:** The delay avoids a write per keystroke while preserving a low
loss window. Explicit boundaries protect navigation and shutdown paths.

**Constraint:** Force termination or power loss during the debounce window is a
known limitation; no implementation may claim stronger durability without an
additional local draft journal.

**Affected specifications:** 05, 07.

## D-027: Clipping deletion

**Status:** Approved

**Decision:** Deleting a clipping is an explicit confirmed action that removes
its title, note, canonical managed asset, and derived thumbnail cache. It never
deletes the source page or edition.

**Rationale:** The clipping is one aggregate. Retaining orphan notes or images
would confuse ownership and cleanup.

**Constraint:** Deletion uses a recoverable asset state/rename flow so a crash or
database error does not silently leave an unreadable row.

**Affected specifications:** 02, 06.

## D-028: Missing source versus missing asset

**Status:** Approved

**Decision:** These are distinct states:

- **Source unavailable:** The clipping and note remain fully usable; `Open
  source` is disabled with explanatory copy.
- **Canonical asset missing/corrupt:** The note and provenance remain visible,
  the clipping header shows an integrity warning, and repair/recovery is attempted
  or offered. The row is not silently removed.

**Rationale:** Source removal is expected lifecycle behavior. Canonical asset
loss is a data-integrity problem and must be surfaced differently.

**Affected specifications:** 02, 05, 06.

## D-029: Deferred V1 extensions

**Status:** Deferred

**Decision:** OCR, AI summaries, translation, embeddings, semantic search,
annotations, multiple attachments, tags, folders, favorites, reminders,
spaced repetition, export bundles, sharing, and synchronization are excluded
from V1.

**Rationale:** They require independent product, privacy, model, indexing,
schema, or attachment decisions. The V1 aggregate preserves sufficient stable
provenance for later additions without implementing them now.

**Affected specifications:** All.

## D-030: Screenshot-based canonical clipping

**Status:** Rejected

**Decision:** A browser or operating-system screenshot is not accepted as the
canonical V1 clipping.

**Rationale:** It loses resolution when zoomed out, includes display-scale and
browser-rasterization effects, may bake in dim/inverted tone, and can capture
UI overlays. A future **Export current appearance** command could be separately
specified, but it must not replace the managed source crop.

**Affected specifications:** ADR-002, 03, 04.

## D-031: Detached managed cleanup enumeration

**Status:** Approved

**Approval:** Approved by the product owner on 2026-08-09 for the Phase 1
review correction.

**Decision:** V1 deferred cleanup waits for a five-second startup quiet period,
then runs only in one detached blocking lifecycle task and may completely
enumerate each application-managed clipping category. Critical database-driven
recovery of `creating` and `delete_pending` rows remains synchronous. The
deferred task does not claim that directory enumeration or inspection is
bounded. Actual `ReadDir` items consumed are counted, while filesystem mutation
attempts remain limited to 32 per managed category per launch.

The managed categories are staging, canonical assets, trash, quarantine, and
derived clipping thumbnails. Traversal streams entries without sorting or
retaining a directory-sized filename collection. Cleanup does not persist or
resume a filename cursor because stable `std::fs::read_dir` provides neither a
defined ordering nor a portable durable directory position.

**Supersedes:** The earlier interpretation of `RECOVERY-004` that all cleanup
inspection/enumeration work was bounded per launch. It does not supersede the
24-hour orphan grace period, seven-day quarantine retention, containment
checks, path redaction, or the prohibition on scanning user newspaper download
folders.

**Product rationale:** The approved flat managed-path layout has no portable
indexed directory cursor. Complete detached enumeration gives honest,
repeatable classification and prevents known/fresh filename filtering from
hiding later entries, while the mutation cap bounds destructive work.

**Persistence and migration impact:** No schema or application schema-version
change. The misleading `clipping_cleanup_cursor_v1` setting implementation and
its writer calls are removed. A stale value left by an earlier draft is ignored.

**Security and privacy impact:** Enumeration remains limited to backend-derived
managed roots. Every mutation retains regular-file/directory, reparse/symlink,
canonical-containment, exact-ID, and age checks. Diagnostics contain counts and
safe classifications only.

**Test and release-gate impact:** Phase 1 tests must count actual iterator items,
prove at most 32 mutation attempts per category, prove repeated passes reach
removable leftovers, and record 500-entry and 5,000-entry wall time plus
approximate/peak memory evidence. Production composition must prove the quiet
period precedes the blocking worker and no UI or startup thread waits for
enumeration.

**Backward compatibility and rollback:** Managed paths and database rows are
unchanged. Reverting before release restores the prior cleanup implementation;
after release, a forward fix is preferred. Cleanup rollback never deletes the
managed root or scans user download folders.

**Affected specifications:** 02 and 07.

## D-034: Clipping-note recovery checkpoints and cooperative exit

**Status:** Approved

**Approval:** Approved by the product owner on 2026-08-10. The product owner
delegated approval of the recommended durability defaults after separately
approving the close-X and recovery-envelope choices.

**Decision:** Keep D-026's 800 ms canonical autosave debounce and add a 5-second
maximum canonical-save wait during continuous typing. Persist a separate native
SQLite recovery checkpoint after 500 ms of quiet time and at least every 2
seconds during continuous typing. The recovery-only limits are 4 KiB of UTF-8
title bytes and 4 MiB of UTF-8 Markdown bytes; canonical validation remains 800
title bytes and 2 MiB Markdown.

Window X prevents native close, completes the durability handshake, and hides
the existing main WebView only on success. Tray **Quit**, ordinary application
exit, and updater-controlled exit use the same handshake and exit only on
success. A canonical revision conflict may proceed only when the exact newest
visible draft has a matching acknowledged recovery checkpoint; the next launch
must offer both versions explicitly. Failure, an uncheckpointed/stale conflict,
missing/stale acknowledgement, or timeout keeps the application alive and
restores/focuses the main window. Timeout never means discard.

The journal stores one sequence-aware checkpoint per clipping. It is not
searchable and does not update canonical metadata or FTS. A canonical save
clears only the matching acknowledged checkpoint in the same SQLite savepoint.
Recovery after renderer/process loss is explicit and revision-aware; a stale or
different writer session cannot overwrite or clear a newer draft silently.

**Supersedes:** D-026 only where it states that V1 has no second local draft
journal and accepts loss within the debounce window. D-026's canonical debounce,
single in-flight save, optimistic revision, and explicit flush boundaries remain
binding.

**Product rationale:** Ordinary navigation already blocks on failed saves, but
React cleanup cannot safely await persistence and the current native close/exit
paths do not cover every destructive lifecycle edge. A bounded native
checkpoint closes that gap without writing SQLite on each keystroke or turning
the Tiptap editor component into a persistence/lifecycle owner.

**Persistence and migration impact:** Schema version 6 adds
`newspaper_clipping_note_drafts` after the existing verified backup. The table
has a clipping foreign key with `ON DELETE CASCADE`, canonical base revision,
writer session/sequence, title, Markdown, and update time. It has no FTS trigger.
Fresh, v5-to-v6, current, failure, and future-version cases must pass the normal
application migration boundary.

**Security and privacy impact:** Draft bytes stay in the local application
database and never enter logs, diagnostics, search, telemetry, or filesystem
paths. Backend byte limits and typed path-free errors are authoritative. No
browser storage, second writer, new network surface, or new dependency is
approved.

**Test and release-gate impact:** Add deterministic controller, migration,
repository, conflict, sequence, close-X, tray Quit, updater, timeout, crash
recovery, browser, performance, and installed Windows lifecycle evidence. The
structural gate enforces module-size and ownership budgets from the durability
work order. No ignored durability test is accepted.

**Backward compatibility and rollback:** Older binaries cannot open schema 6.
Rollback therefore preserves recovery rows and uses a forward fix; it never
downgrades `user_version` or drops a draft automatically. UI/native wiring may
be disabled while leaving canonical saves and recoverable rows intact.

**Implementation boundary:** Phase 4C is a separately reviewed durability
slice after Phase 4B behavior is available. It follows
`docs/work-orders/newspaper-clipping-note-durability-plan.md` and may not grow
the canonical TSX/service owners beyond that work order's budgets.

**Affected specifications:** README, 02, 05, 06, 07, and 08.

## Change procedure

A proposed decision change must include:

1. New decision ID and status `Proposed`.
2. The decision it supersedes, if any.
3. Product rationale.
4. Persistence and migration impact.
5. Security and privacy impact.
6. Test and release-gate impact.
7. Backward-compatibility and rollback impact.
8. Affected specification links.
9. Reviewer and approval date.

Until approved, implementation follows the latest approved entry and stops if
that is impossible.
