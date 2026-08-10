# Newspaper Clippings Phase 5 navigation and lifecycle work order

Status: Automated implementation complete; native UAT pending
Date: 2026-08-10
Branch: `codex/newspaper-clippings-phase5-lifecycle`

## 1. Outcome

Phase 5 completes the lifecycle around an already durable clipping:

1. open its exact source edition and page by stable IDs;
2. show the persisted crop rectangle as a three-second, non-interactive reader
   highlight and return to the exact clipping;
3. keep source-unavailable and canonical-asset-unavailable states distinct;
4. delete one clipping through the existing crash-recoverable state machine
   without touching its source edition or sibling clippings; and
5. preserve every clipping, note, revision, canonical asset, and clipping
   thumbnail when World Journal download data is reset.

This phase does not add heuristic relinking, recropping, OCR, tags, export,
sync, or a second navigation framework.

## 2. Confirmed current ownership

### 2.1 Existing behavior to reuse

- `App.tsx` already owns top-level provider selection and the canonical clipping
  note flush guard.
- `NewspaperClippings.tsx` owns selected detail identity and prevents switching
  clippings before the current draft flushes.
- `NewspaperLibrary.tsx` owns library query/filter/virtual-scroll state and the
  currently opened `NewspaperReader`.
- `NewspaperReader.tsx` owns exact page IDs, reading-progress flushing, zoom,
  virtual page layout, selection, and clipping creation.
- `ClippingService::delete` already marks a row `delete_pending`, moves its
  canonical asset through managed trash, deletes the row transactionally, and
  leaves crash recovery evidence on ambiguous failures.
- startup recovery already completes retained `delete_pending` rows.
- reset already nulls clipping source IDs before deleting reset-owned newspaper
  rows and has persistence tests proving note/revision preservation.
- `source_available` is derived by current joins; no mutable availability flag
  is stored.

### 2.2 Missing integration

- clipping detail responses do not expose stable source IDs, media-version
  snapshot, or normalized crop geometry needed for exact navigation;
- there is no exact `get_newspaper_library_item(jobId)` command;
- app/provider state carries only a pending clipping ID, not a typed one-shot
  reader/clipping return target;
- the reader cannot accept an exact initial page/highlight or report
  `Back to clipping`;
- the existing delete service is not exposed as a Tauri command or UI action;
- canonical asset recovery is internal/startup-only and has no bounded targeted
  user command;
- reset invalidates the library but does not emit a clipping `source_changed`
  invalidation or use the approved preservation copy in every UI message.

## 3. Decisions

### D-P5-001: One provider navigation owner

Add a bounded `newspaper-navigation.ts` module containing discriminated target
types and pure target-consumption helpers. `App.tsx` stores only the current
one-shot target and continues to own top-level view transitions. It must not
own reader manifests, clipping data, or editor documents.

### D-P5-002: Exact lookup before navigation

`get_newspaper_library_item(jobId)` performs one exact backend lookup and only
returns readable completed/partial jobs. The frontend then verifies the exact
page in the reader manifest. It never searches a loaded virtual page or falls
back to a page index.

### D-P5-003: Preserve search and scroll state

Opening a clipping or source does not remount the provider workspace. Existing
library/clipping component state stays mounted where practical; return targets
carry stable IDs and focus intent, not copied list data. Stale one-shot targets
are cleared only after success or typed unavailable handling.

### D-P5-004: Highlight is a reader overlay, not clipping selection

Create a small `NewspaperSourceHighlight.tsx` plus pure geometry/lifecycle
helpers. The overlay uses normalized percentages, `pointer-events: none`, and
no clipping operation ID. It expires after three seconds and clears on scroll,
zoom, page change, pan/click zoom, Clip mode, Escape, or Back. Reduced motion
removes transitions.

### D-P5-005: Delete delegates to the existing state machine

The command validates ID/revision and calls `ClippingService::delete`. UI must
flush before opening confirmation, default focus to Cancel, require a fresh
click, keep the item visible while deleting, and re-fetch on ambiguous failure.
No frontend filesystem operation is allowed.

### D-P5-006: Targeted asset recovery is identity-bound

`recover_newspaper_clipping_asset(clippingId)` may inspect only the row's exact
canonical, operation-owned staging, trash, and quarantine identities through
existing managed-layout validators. It may never recursively search a download
tree, use filename similarity, or recrop from current source bytes.

### D-P5-007: Reset preservation is explicit at every layer

Keep the current transactional unlink order. After commit, emit clipping
invalidation reason `source_changed` in addition to existing library reset
invalidation. UI confirmation and success copy must say that clipping images
and notes are preserved. Reset never clears clipping roots or clipping caches.

## 4. Implementation slices

### Slice A: response and exact lookup contracts

1. Extend clipping detail with nullable `sourceJobId`, `sourcePageId`,
   `sourceMediaVersionSnapshot`, and `normalizedRect`.
2. Add exact library-item repository/service/command lookup by job ID.
3. Add safe unavailable mappings for missing job/page/not-ready/manifest.
4. Add structural and Rust contract tests; no paths or note content in errors.

### Slice B: navigation and transient highlight

1. Add provider navigation target types/helpers outside `App.tsx`.
2. Thread a reader target through `App` -> `NewspaperView` ->
   `NewspaperLibrary` -> `NewspaperReader`.
3. Verify exact page identity after manifest load, scroll it into view, and draw
   the normalized overlay only after its image connects.
4. Implement `Back to clipping`, Escape behavior, progress flush, and focus
   restoration without changing ordinary `Back to library`.
5. Preserve reader save -> Open note return target and focus intent.

### Slice C: clipping deletion

1. Expose `delete_newspaper_clipping` through a thin blocking command.
2. Emit `newspaper://clippings-invalidated` reason `deleted` after success.
3. Add an overflow action and safe confirmation dialog to clipping detail.
4. Flush before dialog, enforce displayed revision, keep detail visible while
   deleting, then select the logical next/previous cached clipping or gallery.
5. Cover ready/missing assets, revision conflict, sibling/source preservation,
   filesystem/DB failure, and restart recovery.

### Slice D: missing states, recovery, and reset

1. Show the exact source-unavailable message independently of asset status.
2. Show missing/corrupt asset warning while retaining note and provenance.
3. Add bounded `Retry image check` using targeted managed recovery only.
4. Add reset clipping invalidation and approved confirmation/success copy.
5. Verify an open dirty editor survives reset while source controls refresh.

### Slice E: integration and release handoff

1. Run focused navigation/highlight/delete/reset/browser/Rust suites.
2. Run TypeScript, production build, architecture, persistence, UI,
   clipping-library, editor durability, Rust format/clippy/full tests, and
   release regression gates.
3. Launch an owned native dev instance on a non-1420 port.
4. Perform the six Phase 5 native scenarios, stopping before Phase 6 threshold
   ratification or release certification.

## 5. File and performance budgets

- Do not add navigation logic directly to the 4,000+ line `App.tsx`; net growth
  there should be wiring only and no new data-fetching effects.
- Do not put highlight lifecycle into the 1,000+ line reader body; extract the
  overlay and pure target helpers. Reader net growth target: under 120 lines.
- Do not put command orchestration into the 3,800+ line clipping service;
  deletion remains a thin existing call and asset recovery belongs in a
  bounded recovery module.
- Keep `NewspaperClippings.tsx` as the selection owner. Extract confirmation
  UI if it would push the component beyond 250 lines.
- No new runtime dependency.
- No list scan for exact navigation and no recursive filesystem scan for
  recovery.
- Highlight setup is O(1) after exact page lookup; timers/listeners are cleared
  on every target change and unmount.
- Delete and reset remain off the WebView thread.

## 6. Edge cases and conflict gates

- Dirty save failure blocks Open source, Back, another clipping, delete, and
  provider navigation.
- A newer revision arriving after dialog open yields conflict and requires a
  new confirmation.
- A source page at the same numeric index but a different ID must never open.
- Media-version mismatch permits navigation but shows a non-blocking warning.
- A reset/delete invalidation cannot overwrite an editor's local dirty draft.
- Deleting source job/page nulls source IDs without changing title, note bytes,
  revision, timestamps, snapshots, geometry, canonical bytes, or thumbnails.
- Deleting one of two clippings from one page leaves the sibling and page
  byte-for-byte unchanged.
- Missing canonical media never triggers automatic recrop.
- A late manifest/detail request cannot consume a newer navigation target.
- Escape inside Tiptap remains editor-owned and does not trigger reader/search
  navigation.
- Ordinary library-opened readers retain `Back to library` behavior.

## 7. Verification gates

Focused gates must cover:

- exact job/page/clipping ID targets and one-shot generation handling;
- highlight geometry, expiry, early-clear inputs, reduced motion, image error;
- delete revision conflict, state transitions, crash fixtures, source/sibling
  preservation, selection/focus fallback;
- source unlink with foreign keys on and explicitly disabled;
- reset byte-for-byte clipping/note/asset/thumbnail preservation;
- targeted recovery success/rejection without arbitrary search;
- safe messages and absence of paths/note bytes.

Full closeout requires the repository's existing build, architecture,
persistence, UI, clipping library/browser, note durability, Newspaper
performance/browser, Rust format/clippy/full test, release, diff, ignored-test,
and generated-output audits.

## 8. Native UAT

1. Open a clipping's source; verify exact page/highlight and Back to clipping.
2. Save from reader and choose Open note; verify exact detail and focused body.
3. Reset World Journal; verify selected clipping/note/image stay open and only
   source becomes unavailable after restart.
4. Use a disposable missing-asset fixture; verify warning, editable note, and
   bounded Retry image check.
5. Delete one of two same-page clippings; verify sibling and source remain.
6. Repeat keyboard Back/Open source/delete cancel/confirm at supported Windows
   display scaling. Phase 6 owns installed-build certification.

## 9. Local implementation record (2026-08-10)

Implemented on `codex/newspaper-clippings-phase5-lifecycle` without a new
runtime dependency or a persistence-baseline increase:

- stable job/page/clipping navigation targets, exact backend job lookup,
  three-second normalized source highlight, media-version warning, and focused
  `Back to clipping` return;
- preserved clipping search query and gallery scroll context across the source
  round trip;
- revision-guarded clipping deletion through the existing `delete_pending`
  state machine, with Cancel-first confirmation and next/previous cached-item
  selection;
- separate missing-source and missing-asset states plus identity-bound exact
  canonical revalidation; no download-tree scan, remote fetch, or recrop;
- reset and single-job source invalidation while clipping rows, notes, assets,
  thumbnails, and revisions remain owned by the clipping aggregate; and
- bounded navigation, selection, deletion, status, and highlight modules; and
- an image-retry state fix that remounts the verified canonical clipping after
  a prior renderer load failure instead of leaving the source card stuck in
  its local fallback state. The
  fixed durability size gates pass at `App.tsx` 4,444 lines,
  `NewspaperClippingDetail.tsx` 257 lines, and `NewspaperClippings.tsx` 72
  lines.

Verification on the final implementation:

- TypeScript/production build, architecture, UI, persistence (44/44), clipping
  library/lifecycle/reader, note autosave/lifecycle/durability, static
  newspaper performance, and 8/50/500 browser performance: exit 0;
- expanded clipping browser matrix: exit 0, including exact third-page target,
  highlight geometry/expiry, search/source context return, bounded responsive
  thumbnails, successful image retry remount, focus return, delete Cancel, and
  next cached selection;
- editor evaluation: 17/17 checks passed with the documented 90-second local
  navigation override after two setup-only 30-second cold-transform timeouts;
- Rust format and clippy: exit 0 (36 pre-existing warnings); full Rust and
  release Rust: 563 passed, 4 documented pre-existing ignores; and
- composite `verify:release`: exit 0, including 800/800 persistence writes,
  333 ms contention, 2,402 writes/s, and release manifest verification.

Native dev UAT is the remaining Phase 5 exit condition. Destructive reset and
missing-asset fault injection are already automated and should not be performed
against a user's irreplaceable newspaper data merely to repeat the proof.
