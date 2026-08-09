# Newspaper Clippings core workflow implementation plan

**Status:** Planning only — no production implementation is authorized by this
document

**Date:** 2026-08-09

**Target outcome:** A user opens a downloaded newspaper, enters Clip mode,
drags one rectangle over one page image, saves a source-resolution lossless
WebP beneath the source edition's newspaper download destination, opens the new
clipping, and types a note in the approved Tiptap editor with safe autosave.

## 1. Decisions frozen for implementation

- The canonical crop is created by Rust from registered source media, never
  from a WebView screenshot.
- React sends a normalized rectangle and the page's expected media version.
- Canonical media lives under the source job's original newspaper download
  destination in `Newspaper snapshots/<edition>/`. It is not duplicated under
  `LinkVaultData`.
- The proposed collision-safe layout is
  `<destination>/Newspaper snapshots/<sanitized edition name - code>/<publication-date>/<clipping-id>/clipping-v1.webp`.
  The date prevents different issues of one edition from colliding, and the
  clipping ID preserves idempotent create/recovery semantics.
- Rust derives the destination and every relative segment from registered
  source/job data. React never chooses or receives a raw filesystem path.
- Saving creates exactly one clipping aggregate containing one immutable image
  and one initially empty Markdown note.
- The clipping image is a fixed source card above the note editor. It is not an
  editable node in the note body.
- The editor is the exact Tiptap 3.29.2 trio approved by D-024 and is imported
  only by the LinkVault-owned `ClippingNoteEditor` adapter.
- SQLite stores title and plain Markdown. Tiptap/ProseMirror JSON is never
  persisted.
- Keyword search uses SQLite clipping metadata and note Markdown. Folder depth,
  image names, and filesystem enumeration are never part of search.
- Autosave uses the approved 800 ms debounce and optimistic revision contract.
- Screen-reader UAT is not a blocker. Keyboard labels, focus order, pressed and
  disabled states, and visible focus remain required.
- Native Tauri Chinese IME validation is a Phase 4B exit gate, after the real
  autosave and document-switch owners exist.

### 1.1 Required storage-decision amendment

The product owner changed the meaning of "proper folder" on 2026-08-09:
snapshots must be stored in the same selected newspaper download destination,
under `Newspaper snapshots/<edition>/`.

This supersedes ADR-002 and D-009's current `LinkVaultData/newspaper-clippings`
location. It is not a frontend-only Phase 3 change. Phase 2 PR #4 currently
implements one application-data root across asset staging, atomic promotion,
recovery, media protocol reads, cleanup, and database path validation. Do not
merge PR #4 unchanged.

The required Phase 2 amendment must:

- replace D-009 and update ADR-002/specifications before implementation;
- derive the snapshot root from the crop source job's persisted batch
  destination, not from the frontend or the currently selected preference;
- snapshot a backend-only storage-root locator on the clipping row so the asset
  remains resolvable after its source job is deleted;
- keep only a backend-derived relative asset path in ordinary DTOs and never
  expose the absolute root over IPC;
- preserve same-volume staging and atomic promotion inside the chosen snapshot
  root;
- prove edition deletion/reset targets cannot reach the sibling
  `Newspaper snapshots` tree;
- preserve old snapshots at their original root when the user later changes
  the download preference; no implicit migration or cross-volume move;
- define missing/unwritable/removable destination behavior without falling
  back silently to `LinkVaultData` or a different download folder; and
- revise recovery, cleanup, media-protocol, backup/rollback, and path-containment
  tests for multiple registered snapshot roots.

The recommended V1 tradeoff is one canonical file in the snapshot tree, not a
second exported copy. A duplicate managed copy would preserve the old ADR but
would introduce two-file synchronization, deletion, integrity, and storage
semantics that the product did not request.

## 2. Entry gates and branch order

Implementation remains two reviewable PRs. Do not combine them without another
explicit product decision.

1. Phase 2 PR #4 must first be amended for section 1.1, reviewed, and merged.
   Its current implementation commit is
   `0796e78674dd16d4eb6a88f455ac2d63300712c0`; its evidence head is
   `e81e8230aac50372f7b7d22d1482ab35ec18943a`.
2. Phase 4A PR #5 must be reviewed and merged. Its approved decision head is
   `559d78e22ea80caa5d6a81fbe7aa5cd1f1070f49`.
3. Phase 3 branches from the main commit containing Phase 2.
4. Phase 4B branches only after Phase 3 and Phase 4A are both on main.

Before either implementation branch changes code:

```powershell
git status --short --branch
git rev-parse HEAD
git diff --check
git merge-base --is-ancestor <required-merge-commit> HEAD
```

Stop if the required merge is absent, the worktree is dirty with unrelated
changes, or the live owner map materially differs from this plan.

## 3. Current owner map

### Reader and application owners

- `apps/desktop/src/App.tsx`
  - Owns the `AppView` union, World Journal sidebar children, and top-level view
    composition.
  - Phase 4B adds the `newspaper-clippings` view and the pending clipping target
    used by the Reader's **Open note** action.
- `apps/desktop/src/components/newspaper/NewspaperView.tsx`
  - Owns provider-level mode composition.
  - Phase 4B either adds a `clippings` mode or mounts a dedicated
    `NewspaperClippings` component at the same boundary; clipping state must not
    be hidden inside download-form state.
- `apps/desktop/src/components/newspaper/NewspaperLibrary.tsx`
  - Owns the edition list and selected `readerItem`.
  - Threads a narrowly typed clipping capability and `onCreated` callback into
    the Reader. Phase 3 keeps production capability disabled; Phase 4B enables
    it through normal composition.
- `apps/desktop/src/components/newspaper/NewspaperReader.tsx`
  - Owns the virtualized three-page reader, page image nodes, zoom, pan,
    click-zoom, pointer capture, progress flushing, tones, and page navigation.
  - Its existing pointer handlers are the sole gesture owner. Clip mode must
    take priority inside these handlers before pan/click-zoom state mutates.
- `apps/desktop/src/index.css`
  - Owns Newspaper library/reader styling and responsive/high-contrast states.

### Crop and persistence owners arriving from Phase 2

- `apps/desktop/src/components/newspaper/newspaper-api.ts`
  - Already defines `NormalizedCropRect`,
    `CreateNewspaperClippingRequest/Response/Failure`, and
    `createNewspaperClipping`.
- `apps/desktop/src-tauri/src/providers/newspaper/commands.rs`
  - Already exposes the thin async `create_newspaper_clipping` command.
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_service.rs`
  - Owns create, optimistic note update, delete, list, and detail services.
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_repository.rs`
  - Owns list/detail projections, excerpts, optimistic revisions, source
    availability, and clipping persistence.
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_assets.rs`
  - Owns the snapshot-root registry, canonical/derived-thumbnail layout,
    staging/promotion, containment, and protocol-safe reads after the Phase 2
    storage amendment.

Phase 4B must not create a second repository, database connection policy,
asset root, crop command, or note data model.

### Editor owner arriving from Phase 4A

- `apps/desktop/src/components/newspaper/ClippingNoteEditor.tsx`
  - Sole owner of Tiptap imports and editor lifecycle/composition guards.
- `apps/desktop/src/components/newspaper/clipping-note-editor-markdown.ts`
  - Owns the V1 Markdown/link normalization boundary.

The editor adapter deliberately does not own Tauri calls, autosave, clipping
selection, or application navigation.

## 4. Phase 3 — Reader crop interaction

### 4.1 Allowed scope

Implement and verify the complete Reader selection/save workflow against the
amended Phase 2 command, but keep the production capability disabled until
Phase 4B provides the Clippings view and **Open note** destination.

No Tiptap import, Clippings list/detail UI, autosave, deletion, source-return
navigation, schema change, dependency change, or release-version change is
allowed.

### 4.2 Planned files

Modify:

- `apps/desktop/src/components/newspaper/NewspaperReader.tsx`
- `apps/desktop/src/components/newspaper/NewspaperLibrary.tsx`
- `apps/desktop/src/components/newspaper/newspaper-api.ts`
- `apps/desktop/src/index.css`
- `apps/desktop/package.json` only for narrowly named verification scripts

Add:

- `apps/desktop/src/components/newspaper/newspaper-clipping-geometry.ts`
- `apps/desktop/src/components/newspaper/newspaper-clipping-state.ts`
- `apps/desktop/src/components/newspaper/NewspaperClippingSelectionOverlay.tsx`
- focused Node/browser verification scripts following existing repository
  patterns

Do not modify Rust unless a concrete mismatch is proven against the merged
Phase 2 contract. A contract mismatch stops the phase for review.

### 4.3 Pure geometry seam

`newspaper-clipping-geometry.ts` contains no React, Tauri, timers, DOM mutation,
or global state. It should expose deterministic helpers for:

- clamping a client point to an image `DOMRect`;
- building an order-independent rectangle from start/current points;
- rejecting non-finite, zero-area, or below-minimum display selections;
- converting the clamped display rectangle into normalized `x/y/width/height`;
- recalculating from current layout while preserving source page identity;
- checking epsilon-only boundary noise without accepting material overflow.

Unit fixtures cover all four drag directions, every edge/corner, zoom levels,
scroll offsets, fractional CSS pixels, resized layout, degenerate rectangles,
and normalized output invariants. Rust remains authoritative for the final
32-pixel minimum and exact floor/ceil source geometry.

### 4.4 Explicit Reader state machine

Use a reducer or equivalently exhaustive transition function with these states:

```text
browse
clip-selecting
clip-drawing(pointerId, pageId, mediaVersion, start, current)
clip-confirming(pageId, mediaVersion, normalizedRect)
clip-saving(operationId, pageId, mediaVersion, normalizedRect)
```

Required invariants:

- `browse` is the only state that permits existing pan/click-zoom gestures.
- Entering Clip mode cancels any incomplete pan and releases pointer capture.
- One pointer can own one selection on one eligible image.
- Pointer move updates render state at most once per animation frame.
- Pointer cancel, lost capture, window blur, unmount, and Escape cleanly release
  selection locks.
- Confirmation freezes geometry. Wheel/touch scrolling, zoom, tone changes,
  and page navigation cannot silently move the confirmed target.
- Saving has exactly one retained operation ID and cannot double-submit.
- Leaving any clipping state restores ordinary reader behavior.

### 4.5 Toolbar and keyboard behavior

- Add a scissors **Clip** button with `aria-pressed` reflecting active mode.
- Unmodified `C` toggles selection only when the Reader canvas owns the
  interaction and the event target is not an input, select, textarea,
  contenteditable node, dialog control, or future note editor.
- `Ctrl/Cmd+C` remains Copy; repeated keydown is ignored.
- Escape priority is saving guard -> confirmation -> drawing/selection ->
  existing Reader close behavior.
- Confirmation exposes **Save clipping**, **Redraw**, and **Cancel** with a
  stable keyboard focus order.

### 4.6 Pointer and overlay integration

The current Reader canvas handlers must branch on clipping state before
creating `panGestureRef` state. Do not add a second competing pointer listener
tree.

On eligible image pointer-down:

1. Capture the page ID, `mediaVersion`, image node, and current image rect.
2. Capture the pointer on the stable Reader interaction surface.
3. Clamp start/current points to the image.
4. Render one overlay above the native `<img>`; never draw or copy the page into
   a full-page canvas.
5. On pointer-up, normalize geometry and transition to confirmation.

The overlay is presentation-only. `NewspaperReader` owns state, command
invocation, manifest refresh, and navigation callbacks.

### 4.7 Save orchestration

- Generate `crypto.randomUUID()` exactly once on the first Save activation.
- Invoke `createNewspaperClipping` with operation ID, page ID, captured media
  version, and normalized rectangle.
- Disable duplicate controls while saving but keep the Reader mounted.
- On success:
  - clear selection state;
  - return to browse mode at the same reading position;
  - show edition/date/page success copy;
  - call `clippingCapability.onCreated(clippingId)` only when supplied;
  - expose **Open note** only after Phase 4B provides a real destination.
- On `SOURCE_MEDIA_STALE`, retain visible geometry, refresh the authoritative
  manifest, rebind only after the user confirms the refreshed page, and use a
  new operation ID only when the old request is known not to have created a
  clipping.
- On too-small, security/integrity, retryable storage, or ambiguous failures,
  preserve or discard selection exactly as specification 04 requires. Never
  silently crop a different page/version.

### 4.8 Phase 3 verification

Automated proof must cover:

- pure geometry and transition tables;
- Clip button/C shortcut/editable-target safety;
- pan, zoom, page navigation, tone, and progress isolation;
- top-left, bottom-right, reverse, outside-image, resize, cancel, lost-capture,
  and window-blur selections;
- duplicate Save and idempotent retry;
- typed stale/too-small/security/retryable error UI;
- source jobs using two different download destinations, with each canonical
  crop remaining under its own original `Newspaper snapshots` root;
- changing the current download preference after creation without relocating
  or orphaning an existing clipping;
- 8/50/500-page manifests with the existing mounted-image bound unchanged;
- no new full-page canvas or screenshot path;
- keyboard focus/labels and light/dark/high-contrast visuals.

Manual Phase 3 exit smoke remains limited to crop geometry and Reader behavior
at 100/125/150/200% display scaling. It does not require the editor.

Minimum command set from a clean committed Phase 3 worktree:

```powershell
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:newspaper-performance
npm.cmd --prefix apps\desktop run verify:newspaper-performance-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
npm.cmd --prefix apps\desktop run verify:newspaper-clippings-browser
cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
npm.cmd --prefix apps\desktop run verify:release
git diff --check
```

### 4.9 Suggested Phase 3 commit slices

1. Pure geometry and reducer with exhaustive fixtures.
2. Overlay, toolbar, keyboard, and pointer-priority integration.
3. Create-command orchestration, typed failures, browser harness, performance
   proof, and evidence docs.

Stop after Phase 3 review readiness. Do not start Phase 4B in the same branch.

## 5. Phase 4B — Clippings view and Tiptap notes

### 5.1 Allowed scope

Production-enable the complete save-and-review path:

- third World Journal sidebar child named **Clippings**;
- paged/virtualized clipping list;
- selected clipping detail with fixed source image card;
- title and Tiptap Markdown editor;
- 800 ms autosave, flush boundaries, failures, and revision conflicts;
- Reader success-toast **Open note** navigation;
- lazy editor loading and native Tauri IME verification.

Source-return navigation, transient source highlight, deletion, reset UI,
missing-source lifecycle actions, OCR, AI, annotations, tags, export, sync, and
release-version changes remain later phases.

### 5.2 Backend IPC gap to close

The repository and service already implement list, detail, optimistic note
update, and deletion, but only create is exposed to React. Phase 4B adds thin,
safe, camelCase commands and DTOs for:

```text
get_newspaper_clippings_page
get_newspaper_clipping
update_newspaper_clipping
ensure_newspaper_clipping_thumbnail
```

Planned backend files:

- `apps/desktop/src-tauri/src/providers/newspaper/clipping_models.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_service.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/commands.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/mod.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- thumbnail-generation helper only under the existing clipping asset/service
  boundary

Rules:

- Commands delegate to `ClippingService`; they do not open ad-hoc connections.
- Raw SQLite, filesystem paths, and internal error causes never cross IPC.
- List DTOs contain excerpts and versioned thumbnail URLs, never full Markdown
  or canonical image bytes.
- Detail returns one full title/Markdown document and one versioned canonical
  media URL.
- Update validates title/Markdown, uses `expectedRevision`, and maps conflict,
  missing, not-editable, and safe database errors without data loss.
- Thumbnail generation is bounded, cached, versioned by asset version, and
  best-effort. It never mutates the canonical clipping image.
- Add command registration and binding-contract checks without hand-editing a
  generated bindings file.

### 5.3 Frontend API and navigation

Extend `newspaper-api.ts` with exact list/detail/update/thumbnail request and
response types plus safe error discrimination.

`App.tsx` changes:

- Add `newspaper-clippings` to `AppView`.
- Add the third **Clippings** World Journal child.
- Own `pendingClippingId` for Reader-to-detail navigation.
- Render the dedicated Clippings surface without loading Tiptap on unrelated
  routes.

Thread one callback through the existing composition path:

```text
App
  -> NewspaperView(mode="library", onOpenClipping)
  -> NewspaperLibrary(clippingCapability)
  -> NewspaperReader(onCreated)
  -> success toast action
  -> App switches to Clippings with pendingClippingId
```

Phase 4B removes the temporary disabled capability and enables Clip through
normal product composition. Do not create a permanent feature flag.

### 5.4 Clippings list and detail components

Add under `apps/desktop/src/components/newspaper/`:

- `NewspaperClippings.tsx`
- `NewspaperClippingList.tsx`
- `NewspaperClippingDetail.tsx`
- `NewspaperClippingSourceCard.tsx`
- `clipping-note-save-controller.ts`
- focused test/support files

List requirements:

- Sparse paged model with stable ID keys and deterministic selection.
- Default `updated_desc`; approved search and sort values only.
- Reuse the existing virtualizer pattern.
- Request thumbnails only for visible rows; dedupe/bound generation requests.
- Preserve selection across refresh when the ID remains visible.
- Never load canonical images or full Markdown for all rows.
- Explicit 0/1/8/50/51/500 row tests and loading/error/empty states.

Search requirements:

- Render the specified `Search titles, notes, editions, dates, or pages` field
  in the Clippings list and debounce backend queries by 200 ms.
- Search server-side across title, full `note_markdown`, edition name/code,
  publication date, and page number. Do not filter only the currently loaded
  virtual rows.
- Trim and cap input at 200 UTF-8 characters and preserve literal `%`, `_`,
  backslash, apostrophe, punctuation, English, and Chinese queries through the
  existing parameterized escaped-`LIKE` repository contract.
- Reset paging for a new query, reject stale query generations, and allow a
  direct **Open note** target to load by ID even when excluded by the current
  search.
- Search never walks `Newspaper snapshots`, opens WebP files, or depends on the
  storage-root locator. Moving from `LIKE` to SQLite FTS later must remain an
  internal repository/index migration behind the same list API.
- Keep D-019's simple substring search for V1 and record 8/50/500 response and
  visible-update timings. Propose FTS only after a larger representative
  fixture demonstrates a measured responsiveness problem; do not introduce an
  FTS synchronization lifecycle speculatively.

Detail requirements:

- Abort/ignore stale detail responses by clipping ID and request generation.
- Render one fixed source card above title/editor using the versioned media URL.
- Handle ready versus missing canonical asset without deleting the note.
- The ordinary source card cannot be removed or edited through Tiptap.
- Expanded image viewing, if retained, is a bounded detail-only overlay and not
  a new persistent canvas architecture.

### 5.5 Editor integration

- Lazy-import only the LinkVault-owned `ClippingNoteEditor` adapter.
- No production file outside the adapter imports `@tiptap/*`.
- Pass persisted Markdown only when `documentId` changes.
- Never call `setContent` for ordinary parent acknowledgements/rerenders.
- Flush the prior draft before switching clipping IDs.
- Preserve composition guards and one committed ready callback.
- Keep raw HTML, MDX, images, tables, code, task-list markers, unsafe links, and
  file/image paste outside the V1 persisted subset.

### 5.6 Autosave controller

Implement a pure, separately tested controller owning:

```text
documentId
persistedTitle / persistedMarkdown / persistedRevision
draftTitle / draftMarkdown
status: clean | dirty | saving | failed | conflict
one debounce timer
one in-flight update
queued-latest draft
```

Rules:

- Mark dirty only when draft differs from the persisted snapshot.
- Debounce 800 ms after the last stable editor/title change.
- Never invoke Tauri synchronously from an editor transaction.
- Permit one update in flight. If the draft changes while saving, queue only
  the latest draft and schedule it after acknowledgement.
- On success, advance persisted values/revision and remain dirty if a newer
  draft exists.
- On safe failure, preserve the draft and expose Retry.
- Flush before clipping switch, route change, editor unmount, application blur,
  and cooperative native close.
- A failed flush blocks navigation and offers Stay/Retry; no draft is silently
  discarded.
- Empty/oversized title or Markdown blocks autosave with inline validation.

### 5.7 Revision conflict behavior

On `CLIPPING_REVISION_CONFLICT`:

1. Stop ordinary autosave.
2. Keep the local title/Markdown draft intact.
3. Fetch the latest saved detail once.
4. Offer exactly:
   - **Keep my changes** — resubmit local draft against latest revision;
   - **Use saved version** — replace local state and reset editor identity;
   - **Copy my draft** — copy safe plain Markdown without changing state.
5. Never retry in a loop or silently overwrite another window.

Test two controllers starting from revision 5: one reaches revision 6; the
other enters conflict with its local draft preserved.

### 5.8 Invalidation and refresh

Emit or reuse one Newspaper clipping invalidation event after create/update and
thumbnail readiness. Event payloads contain only IDs/revisions needed to refresh
visible state. Do not reload the entire list or remount the editor after each
keystroke acknowledgement.

The Reader's successful create should make the new clipping selectable before
the **Open note** action navigates. Detail navigation may fetch directly by ID;
it must not wait for the first list page to contain the item.

### 5.9 Phase 4B verification

Automated proof must include:

- IPC DTO serialization and safe error mapping;
- list/detail/update/thumbnail repository-service-command integration;
- visible-only thumbnails and 8/50/500 list performance;
- keyword search over title/note/provenance, including English, Chinese,
  literal wildcard characters, deep matches, and stale-query rejection;
- source card ready/missing behavior;
- title and Markdown validation boundaries;
- 800 ms debounce, one in-flight save, queued-latest acknowledgement, retry,
  flush, unmount, blur, route change, and cooperative close;
- three conflict actions with local draft preservation;
- document switch isolation and stale detail/update response rejection;
- React 19 Strict Mode, undo/redo, formatting, safe paste/links, dark/light,
  read-only, and lazy chunk isolation;
- Reader save -> success toast -> Open note -> selected detail;
- normal routes do not initialize or fetch the editor chunk;
- no screen-reader blocking gate.

Native Phase 4B Tauri smoke:

- Microsoft Pinyin candidate selection, Enter, Space, arrows, punctuation,
  Backspace, and Escape;
- formatting and undo/redo around committed Chinese text;
- autosave only after stable composition;
- clipping switch after successful flush and failed-flush draft preservation;
- application blur/restore and light/dark themes.

Minimum clean-worktree commands:

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
npm.cmd --prefix apps\desktop run verify:clipping-note-editor-markdown
npm.cmd --prefix apps\desktop run verify:clipping-note-editor
npm.cmd --prefix apps\desktop audit --omit=dev
cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
npm.cmd --prefix apps\desktop run verify:release
git diff --check
```

Every command records commit, OS, exit code, elapsed time, relevant output, and
intermediate failures. Generated dist/browser output is removed before commit.

### 5.10 Suggested Phase 4B commit slices

1. Safe list/detail/update/thumbnail DTOs, commands, and service tests.
2. App navigation, Clippings list, visible-only thumbnails, and source card.
3. Lazy Tiptap detail integration and pure autosave controller.
4. Conflict/navigation guards, Reader Open note enablement, browser/performance
   gates, native IME evidence, and documentation.

## 6. End-to-end acceptance ledger

The core goal is complete only when all rows pass in the real Tauri app:

| Scenario | Required result |
|---|---|
| Enter Clip mode | Clip button and `C` enter one explicit selection state without breaking pan/zoom outside that state. |
| Draw | One rectangle clamps to one page and remains aligned across zoom/tone/scroll. |
| Save | Rust crops registered source pixels and persists one lossless canonical WebP beneath the source job's `<destination>/Newspaper snapshots/<edition>/` tree plus one empty Markdown note. |
| Continue reading | Reader returns to the same position and ordinary gestures recover. |
| Open note | Success toast opens the exact new clipping in the Clippings view. |
| Source card | The saved crop renders above the editor and cannot be removed through note editing. |
| Type note | Tiptap edits safe plain Markdown; Chinese IME works in the integrated Tauri WebView. |
| Autosave | Stable draft saves after 800 ms; route/document/blur/close boundaries flush safely. |
| Reopen | The persisted title, Markdown, image, and revision reload correctly. |
| Find later | A keyword in the title or note finds the clipping through paged SQLite search without scanning the nested snapshot folders. |
| Conflict | A concurrent update preserves the local draft and presents three explicit recovery actions. |
| Scale | Reader selection remains correct at 100/125/150/200%; list remains bounded at 8/50/500 items. |

## 7. Non-negotiable stop conditions

Stop and request review if implementation would:

- start before required PRs are merged;
- expose raw filesystem paths or accept screenshot bytes;
- merge the current Phase 2 storage implementation without replacing ADR-002
  and D-009 for the approved `Newspaper snapshots` location;
- add a second crop/persistence/asset owner;
- import Tiptap outside `ClippingNoteEditor`;
- persist editor JSON or unsupported executable Markdown;
- let pan/click-zoom and clipping own the same pointer simultaneously;
- load full Markdown/canonical images for every list row;
- implement keyword search by walking snapshot folders or client-filtering only
  the currently loaded virtual rows;
- write on every editor transaction;
- discard a failed/conflicting draft;
- combine Phase 3 and Phase 4B in one implementation PR;
- begin Phase 5 deletion/source-navigation/reset work;
- change dependencies beyond the approved Tiptap trio or change the release
  version.

## 8. Planning handoff

This file is the local implementation blueprint, not authorization to begin
coding. The next authorization should name either:

- **Phase 3 implementation**, after the storage-amended PR #4 is merged; or
- **Phase 4B implementation**, only after Phase 3 and PR #5 are merged.

Each phase starts from a fresh live diff/owner audit and ends at its own review
boundary without merging automatically.
