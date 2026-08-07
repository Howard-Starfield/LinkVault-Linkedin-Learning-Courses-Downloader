# Newspaper Clippings V1: navigation, deletion, and reset

**Status:** Proposed

**Primary implementation phase:** Phase 5

**Related decisions:** D-001, D-013, D-016 through D-018, D-021, D-027, D-028

## 1. Purpose

This specification defines app-level Newspaper navigation, deep opening from a
reader save to a clipping note, exact return from a clipping to its source page,
transient source highlighting, back behavior, dirty-note navigation guards,
clipping deletion, source-edition deletion semantics, World Journal reset
preservation, and missing-source/missing-asset recovery presentation.

The current application tracks top-level views in `App.tsx`, and the current
`NewspaperLibrary` owns an opened `NewspaperReader` in local state. Exact
clipping ↔ source round trips require a small provider-level navigation
contract; they must not be approximated by searching the currently loaded
virtual list or by reusing page index without page identity.

## 2. Top-level view contract

The application view union gains:

```ts
type AppView =
  | "downloads"
  | "linkedin-history"
  | "coursera"
  | "coursera-history"
  | "newspaper-download"
  | "newspaper-library"
  | "newspaper-clippings";
```

### FR-NAV-001

Selecting `Clippings` sets `activeView = "newspaper-clippings"` and preserves
normal World Journal expansion behavior.

### FR-NAV-002

The World Journal parent is not itself marked active. Exactly one child receives
active styling.

### FR-NAV-003

The Clippings view is a provider-owned component. App state carries navigation
intent only; it does not own clipping list data, editor document state, or crop
logic.

## 3. Provider navigation target

Conceptual target:

```ts
export type NewspaperNavigationTarget =
  | {
      type: "library";
    }
  | {
      type: "reader";
      jobId: string;
      pageId?: string | null;
      highlight?: NormalizedCropRect | null;
      sourceMediaVersionSnapshot?: number | null;
      returnTarget:
        | { type: "library" }
        | { type: "clipping"; clippingId: string };
    }
  | {
      type: "clipping";
      clippingId?: string | null;
      focusEditor?: boolean;
      returnTarget?: { type: "reader"; jobId: string; pageId: string } | null;
    };
```

The exact storage may use React state/context or a provider workspace component,
but behavior must match this contract.

### FR-NAV-004: Identity over index

Deep navigation always uses `jobId`, `pageId`, and `clippingId`. Page/list indexes
are view-local hints and never durable navigation identifiers.

### FR-NAV-005: One-shot intent

A target is consumed only after the destination confirms the requested entity
was loaded. Stale targets are cleared with typed unavailable behavior; they are
not replayed on every render.

### FR-NAV-006: Dirty guard

Before leaving a clipping detail for library, reader, download, another
provider, or another clipping, the autosave controller completes the flush or
explicit discard contract from specification 05. App state does not switch
views first and attempt saving afterward.

## 4. Reader-created clipping → Open note

### FLOW-NAV-001

After successful reader save:

1. Reader receives the created clipping ID.
2. Reader stays open and shows the actionable toast.
3. Choosing **Open note** asks the application to navigate to:

   ```ts
   {
     type: "clipping",
     clippingId,
     focusEditor: true,
     returnTarget: {
       type: "reader",
       jobId: currentJobId,
       pageId: sourcePageId
     }
   }
   ```

4. Reading progress is flushed through the existing reader close/navigation
   boundary.
5. `activeView` becomes `newspaper-clippings`.
6. The Clippings view loads the exact ID independently of current search/sort.
7. The source card and editor load.
8. The first editable paragraph receives focus after the editor is ready.

### FR-NAV-007

If the new clipping is not in the current list result because of an active
search, the deep target still opens. The view may show a banner:

```text
This clipping is outside the current search.
Clear search to show it in the list.
```

It must not clear the user’s search silently.

### FR-NAV-008

If the clipping creation response is successful but detail load temporarily
fails, keep the clipping ID and offer Retry. Do not create another clipping.

## 5. Clipping → Open source

### FLOW-NAV-002

1. User chooses **Open source** on the fixed source card.
2. Pending title/note changes flush successfully or navigation is blocked.
3. The detail’s non-null source job/page IDs are used.
4. The frontend invokes a direct item lookup if needed:

   ```text
   get_newspaper_library_item
   ```

   Input: `jobId`.
5. The response must represent a readable completed/partial edition and must
   include the exact source page in its reader manifest.
6. `activeView` becomes `newspaper-library` with a reader target containing
   page ID, normalized highlight rectangle, source media-version snapshot, and
   return clipping ID.
7. The reader loads the edition, locates page by ID, and scrolls it into view.
8. The saved rectangle is highlighted non-interactively.
9. Reader back action says **Back to clipping**.

### Direct library-item lookup contract

Conceptual response reuses `NewspaperLibraryItem`. The command queries by exact
job ID and does not depend on current library search, page offset, or loaded row.

### FR-NAV-009

If the job exists but the exact page ID does not appear as completed in the
current manifest, abort source navigation and refresh the clipping as source
unavailable. Do not open the page now occupying the saved numeric index.

### FR-NAV-010

If page media version differs from the clipping snapshot, source navigation is
still allowed because the page identity exists and the normalized region remains
meaningful. Show a non-blocking notice:

```text
This page image has changed since the clipping was saved.
```

The canonical clipping remains unchanged. The transient highlight uses persisted
normalized geometry.

## 6. Transient source highlight

The source highlight is distinct from the clipping selection state.

Conceptual state:

```ts
type SourceHighlight = {
  pageId: string;
  rect: NormalizedCropRect;
  expiresAt: number;
};
```

### FR-HIGHLIGHT-001

After the target page image is loaded and connected:

- Scroll the rectangle into the visible reader viewport with reasonable
  surrounding context.
- Draw a high-contrast, non-editable outline at normalized coordinates.
- Announce `Saved clipping location highlighted` once.
- Keep it for three seconds.

### FR-HIGHLIGHT-002

Remove highlight early when the user:

- Scrolls.
- Changes zoom.
- Changes page.
- Enters Clip mode.
- Begins pan/click zoom.
- Presses Escape/Back.

### FR-HIGHLIGHT-003

The highlight:

- Has `pointer-events: none`.
- Does not dim the rest of the page.
- Does not enter selection confirmation.
- Does not create an operation ID.
- Does not alter reader tone or canonical pixels.
- Honors reduced motion by appearing/disappearing without animation.

### FR-HIGHLIGHT-004

If the highlight cannot be rendered because the page/image never loads, the
reader remains usable and shows a safe notice; source navigation itself does not
crash or redirect.

## 7. Back and return behavior

### From source reader opened by clipping

- Header action: `Back to clipping`.
- `Escape` in ordinary browse mode follows the same return target.
- Flush reading progress.
- Return to `newspaper-clippings` with the same clipping selected.
- Restore clipping list scroll/query/sort and detail scroll where practical.
- Do not auto-focus editor unless it had focus before source navigation; restore
  focus to `Open source` by default.

### From source reader opened by library

Existing `Back to library` behavior remains unchanged.

### From clipping opened by reader toast

A detail-level `Back to newspaper` action may return to the remembered reader
page while that return target remains valid. Sidebar navigation remains the
primary general escape route. It must not create a browser-history stack that
loops between two stale targets.

### FR-NAV-011

Return targets are cleared when their entity is deleted or reset. A missing
return target falls back to the appropriate library/clippings view with a clear
message rather than a blank reader.

## 8. Source availability derivation

A clipping source is available only when all are true:

- `source_job_id` is non-null and resolves.
- `source_page_id` is non-null and resolves to that job.
- Page status is completed.
- Reader manifest/media URL can be produced.

### FR-SOURCE-LIFE-001

Do not persist an independently mutable `source_available` column. Detail/list
queries derive it from joins/current page state.

### FR-SOURCE-LIFE-002

Source availability changes emit `newspaper://clippings-invalidated` with reason
`source_changed`, but do not increment user note revision or change the clipping
updated timestamp.

### FR-SOURCE-LIFE-003

Source deletion preserves all provenance snapshots and canonical asset data.

## 9. Source edition/job deletion

Any command that deletes a Newspaper job or clears pages must preserve
clippings.

### FR-SOURCE-DELETE-001

With foreign keys enabled, `ON DELETE SET NULL` performs source unlinking. The
implementation also explicitly clears matching clipping source IDs before bulk
reset deletion so preservation remains deterministic and testable even if a
legacy test connection has foreign keys disabled.

Conceptual reset step before page/job deletion:

```sql
UPDATE newspaper_clippings
SET source_page_id = NULL,
    source_job_id = NULL
WHERE source_page_id IS NOT NULL
   OR source_job_id IS NOT NULL;
```

For one-job deletion, scope the update to that job/page set.

### FR-SOURCE-DELETE-002

Unlinking source IDs does not change:

- Title.
- Note Markdown.
- Revision.
- `created_at` or `updated_at`.
- Canonical asset/checksum/version/state.
- Provenance snapshots.
- Crop geometry.

### FR-SOURCE-DELETE-003

After deletion, open Clippings views refresh source badges without remounting or
losing current note drafts.

## 10. World Journal reset integration

The current reset explicitly deletes read pages, reading progress, optimization
tasks, front-page thumbnail records, pages, events, schedules, settings, jobs,
and batches while preserving edition catalog rows. V1 adds clipping preservation
as another explicit invariant.

### Required reset order

Inside the existing reset transaction:

1. Count clippings for diagnostics/test evidence when needed.
2. Set all clipping `source_page_id` and `source_job_id` values to null without
   changing revision/timestamps.
3. Delete existing reset-owned dependent rows in safe order.
4. Delete pages/jobs/batches.
5. Do not delete `newspaper_clippings`.
6. Commit.

Outside the transaction:

- Clear the existing front-page newspaper thumbnail cache as before.
- Do not clear `LinkVaultData/newspaper-clippings`.
- Do not clear clipping thumbnail cache.
- Emit clipping invalidation reason `source_changed`.

### FR-RESET-001

The reset confirmation displays the preservation copy from specification 01.

### FR-RESET-002

The reset success result does not say “all World Journal data removed.” It may
say:

```text
World Journal download data was reset.
Your saved clippings and notes were preserved.
```

### FR-RESET-003

Reset counts may remain backward-compatible. If a preserved-clipping count is
added, it is additive and named clearly (`clippingsPreserved`), not reported as
removed.

### FR-RESET-004

Reset while the Clippings view is open:

- Does not close the selected detail.
- Does not clear title/note/editor history.
- Updates source state to unavailable after transaction success.
- Disables Open source.
- Keeps canonical image loaded or reloadable.

### AC-RESET-001

Given ready and missing clippings with non-empty Chinese/English notes

When World Journal reset runs

Then every clipping row, revision, note byte sequence, canonical asset byte
sequence, checksum, thumbnail, and provenance snapshot is preserved

And source IDs become null

And all previously reset provider tables are cleared as before

And database integrity checks pass.

## 11. Delete clipping command

Command:

```text
delete_newspaper_clipping
```

Request:

```ts
export type DeleteNewspaperClippingRequest = {
  clippingId: string;
  expectedRevision: number;
};
```

Response:

```ts
export type DeleteNewspaperClippingResponse = {
  clippingId: string;
  deleted: true;
};
```

Persistence/file state machine is defined in specification 02.

### FR-DELETE-001: Precondition

Before showing confirmation, flush current draft. If flush fails, the user must
resolve unsaved changes before delete confirmation. Delete never silently drops
a dirty draft.

### FR-DELETE-002: Revision

Deletion uses the displayed latest revision. A conflict refreshes detail and
requires the user to confirm deletion again; it does not delete a changed note
from a stale dialog.

### FR-DELETE-003: Confirmation

Use exact product copy from specification 01. Dialog initial focus is Cancel or
the dialog container according to existing safety conventions; opening the
dialog and pressing Enter must not accidentally confirm.

### FR-DELETE-004: In-flight state

While deletion runs:

- Disable delete and navigation actions for that clipping.
- Keep source card/note visible with `Deleting…`.
- Do not remove the list row optimistically before backend success.

### FR-DELETE-005: Success

- Remove row from local page cache.
- Clear detail state/editor.
- Select logical next/previous item as defined in 05.
- Emit/consume invalidation reason `deleted`.
- Restore focus to the next selected row or Clippings heading.

### FR-DELETE-006: Failure

- Preserve row, canonical image state, and local draft.
- Show `Clipping could not be deleted` with Retry.
- Re-fetch detail/revision when the error is ambiguous.
- Never delete the source edition.

### AC-DELETE-001

Given two clippings from the same source page

When one is deleted

Then only its row, note, canonical asset, and derived thumbnail are removed

And the second clipping and source page remain byte-for-byte unchanged.

## 12. Missing-source state

Exact source-card message:

```text
Original edition is no longer in the newspaper library.
Your clipping and note are still saved.
```

### FR-MISSING-SOURCE-001

- Canonical image displays normally.
- Title/note remain editable.
- Open source is disabled and explains why.
- No repair action attempts to recreate source download automatically.
- Re-registering/redownloading an equivalent edition does not automatically
  relink by date/page in V1; exact source IDs were deleted.

Automatic heuristic relinking is deferred because it could attach a clipping to
a different revision of a newspaper page. A future explicit relink feature
requires a separate decision.

## 13. Missing/corrupt canonical asset state

### Detection paths

- Startup recovery finds incomplete creation.
- Canonical media protocol finds missing/symlinked/invalid/checksum mismatch.
- Detail image load receives a safe unavailable response.
- User activates retry validation.

### FR-MISSING-ASSET-001

The detail preserves note/provenance and shows exact warning from 05. The list
shows an asset-warning badge.

### FR-MISSING-ASSET-002: Retry image check

Offer:

```text
Retry image check
```

This invokes a bounded targeted recovery/validation command:

```text
recover_newspaper_clipping_asset
```

It may:

- Promote a valid retained staging file.
- Restore a matching quarantined operation-owned asset when identity/checksum is
  proven.
- Revalidate a canonical file that was temporarily unavailable.
- Mark ready and increment asset version only if bytes/path version actually
  change under an approved recovery rule.

It may not search arbitrary filesystem locations or use filename similarity.

### FR-MISSING-ASSET-003: No automatic recrop

V1 does not silently regenerate a missing canonical clipping from the current
source page. The source may have changed, and doing so could replace evidence
with different pixels. When no exact recoverable asset exists:

- Keep missing state.
- Allow note copy/edit.
- Allow Open source if available so the user may explicitly create a new
  clipping.
- Keep deletion available.

A future explicit `Recreate from current source` requires a separate decision,
new asset-version semantics, and clear changed-source warning.

### FR-MISSING-ASSET-004

Missing/corrupt asset state does not increment note revision. Recovery that
restores canonical bytes updates asset state/version and invalidates media
caches without changing title/note.

## 14. Navigation and reset error codes

```text
NEWSPAPER_SOURCE_JOB_NOT_FOUND
NEWSPAPER_SOURCE_PAGE_NOT_FOUND
NEWSPAPER_SOURCE_PAGE_NOT_READY
NEWSPAPER_SOURCE_MANIFEST_UNAVAILABLE
NEWSPAPER_NAVIGATION_TARGET_INVALID
NEWSPAPER_NAVIGATION_FLUSH_FAILED
CLIPPING_DELETE_REVISION_CONFLICT
CLIPPING_DELETE_FAILED
CLIPPING_ASSET_RECOVERY_UNAVAILABLE
CLIPPING_ASSET_RECOVERY_FAILED
```

Errors are mapped to safe UI messages and contain no paths or note content.

## 15. Automated test matrix

### Navigation

- Reader save → Open note exact ID.
- Open note while current clipping search excludes new ID.
- Detail load transient failure/retry.
- Clipping → exact source job/page.
- Source page at different virtual/list index.
- Source media version changed warning.
- Source page missing/incomplete.
- Back to clipping and focus restoration.
- Reader opened normally still returns to library.
- Dirty flush success/failure/discard before Open source.
- Rapid target changes do not open stale IDs.

### Highlight

- Correct percentage geometry.
- Scroll into view.
- Three-second expiry with fake timers.
- Early removal on scroll, zoom, page, pan, Clip, Back.
- No interaction/pointer capture.
- Reduced motion.
- Page image load failure.

### Source deletion/reset

- One-job deletion with one and multiple linked clippings.
- Page cascade and job deletion.
- Foreign keys on.
- Legacy/test connection with foreign keys off uses explicit unlink.
- Reset preserves ready/missing rows and canonical bytes.
- Reset while editor clean/dirty/saving.
- Invalidation updates source badges only.
- Edition catalog remains preserved as before.

### Delete clipping

- Ready asset.
- Missing asset.
- Thumbnail present/absent.
- Revision conflict.
- Filesystem rename failure.
- Database failure after trash move.
- Crash fixtures at every delete state.
- Two clippings share source; only target deleted.
- Source edition remains.
- Selection/focus fallback.

### Asset recovery

- Valid staging promotion.
- Valid matching quarantine restoration.
- Mismatched checksum quarantine rejected.
- No arbitrary recursive search.
- Canonical temporarily unavailable then restored.
- No exact asset → remain missing.

## 16. Native UAT scenarios

1. Save clipping, open note, type Chinese note, Open source, Back to clipping.
2. Delete the source edition/reset World Journal, confirm clipping remains.
3. Restart app after reset, confirm source unavailable and note editable.
4. Simulate/copy a test DB with missing canonical file, verify warning and note
   safety.
5. Delete clipping and verify source edition remains readable.
6. Use keyboard only for Open source, Back, overflow/delete dialog, cancel, and
   confirm.
7. Repeat at 100%, 125%, 150%, and 200% Windows display scaling.

## 17. Phase 5 exit gate

Phase 5 is complete only when:

- Phase 4B is merged and green.
- App/provider navigation uses exact IDs and return targets.
- Open source reaches exact page and highlight.
- Back returns to the exact clipping.
- Missing-source and missing-asset states are distinct and tested.
- Clipping deletion and crash recovery are complete.
- Current World Journal reset explicitly preserves clipping rows, notes,
  canonical assets, and clipping thumbnails while unlinking sources.
- All navigation, lifecycle, deletion, reset, and recovery automated/native tests
  pass.
- Existing provider-reset tests and edition-catalog preservation remain green.
- Architecture, persistence, UI, visual, Newspaper performance/browser,
  frontend, Rust, and release gates remain green.
- The coding agent stops. Final threshold ratification and release certification
  belong to Phase 6.
