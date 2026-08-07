# Newspaper Clippings V1: product and interaction contract

**Status:** Proposed

**Related decisions:** D-001 through D-016, D-025 through D-030

**Implementation phase:** Final behavior spans Phases 3 through 5. This document
defines the finished V1 product, not permission to implement all phases at once.

## 1. Purpose

This specification defines what the user sees and how the complete V1 workflow
behaves. It is authoritative for labels, states, navigation intent, keyboard
behavior, confirmations, success/failure feedback, and accessibility. Backend
and persistence details are defined in specifications 02 and 03.

## 2. Primary users and jobs

### US-001: Preserve a region while reading

As a newspaper reader, I can select a meaningful rectangular region of one
page and save it without leaving the edition, so I can continue reading with
minimal interruption.

### US-002: Add personal context

As a reader, every saved clipping automatically has a note area beneath the
source image, so I can record why the section matters without manually creating
or attaching a separate document.

### US-003: Review saved material

As a reader, I can open a dedicated Clippings view, search and sort saved
items, inspect the source image, and edit notes.

### US-004: Return to context

As a reader, I can open the original edition and exact page when the source is
still available, with the saved region briefly highlighted.

### US-005: Retain durable work

As a reader, my clipping image and note remain available when the downloaded
edition is deleted or World Journal download data is reset.

### US-006: Understand failures

As a reader, I receive specific, recoverable feedback when the page changed,
the selected region is too small, the source is unavailable, the asset could
not be saved, the note conflicts with a newer revision, or the canonical image
is missing.

## 3. Information architecture

The expanded World Journal navigation is:

```text
World Journal
├─ Download editions
├─ Newspaper library
└─ Clippings
```

### IA-001: Sidebar item

- Label: `Clippings`
- Icon: Lucide `Scissors`
- Active state: uses the same child-navigation treatment as `Newspaper library`
- Route/view identifier: `newspaper-clippings`
- Accessible name: `Clippings`

### IA-002: Page heading

The Clippings view uses:

```text
Clippings
Saved newspaper sections and your notes
```

The explanatory subtitle may collapse at narrow widths, but the page name is
always visible.

## 4. End-to-end happy path

### FLOW-001: Save a clipping and keep reading

1. The user opens a completed or partial newspaper edition in the reader.
2. The active page is visible.
3. The user presses the **Clip** toolbar button or unmodified `C`.
4. The reader enters selection mode and announces:

   ```text
   Clip mode. Drag over the part you want to save. Press Escape to cancel.
   ```

5. The user drags from any corner to the opposite corner over one page image.
6. The selection is normalized and shown with a clear border; the area outside
   the rectangle is visually subdued.
7. On pointer release, the reader enters confirmation mode and shows:

   ```text
   Save clipping    Redraw    Cancel
   ```

8. The user chooses **Save clipping**.
9. The selection controls become disabled and show `Saving…`.
10. The backend creates the source-resolution managed asset and clipping row.
11. The reader returns to browse mode on the same page, at the same zoom and
    scroll position.
12. A success toast appears:

   ```text
   Clipping saved
   New York · August 7, 2026 · Page A06
   Open note
   ```

13. The user may continue reading or choose **Open note**.

### AC-PRODUCT-001

Given a completed page displayed at any supported zoom and page tone

When the user completes FLOW-001

Then the saved clipping dimensions derive from source pixels

And the reader remains on the same page and position

And the canonical image does not include reader tone, overlay, toolbar, or
selection border

And one clipping note exists in the Clippings view.

## 5. Reader toolbar contract

### UI-READER-001: Clip action placement

The reader’s right-side control group adds:

```text
[ Clip ]
```

Placement rules:

- The action is visible on completed pages.
- It is disabled while no completed page is available.
- It is disabled while the reader manifest is loading.
- It remains visible but disabled during a clipping save.
- It is not hidden in a menu in the baseline desktop layout.
- At constrained widths it may become an icon-only button, but its tooltip and
  accessible name remain `Clip`.

### UI-READER-002: Button states

| Reader state | Button label/appearance | Activation result |
|---|---|---|
| Browse | `Clip` | Enter selection mode |
| Selection, no drag | Active/pressed `Clip` | Exit to browse mode |
| Drawing | Active/pressed, disabled until pointer completes | No second action |
| Confirm | Active/pressed | No toggle; use confirm controls |
| Saving | `Saving…` with progress indicator | Disabled |
| Source page unavailable/failed | `Clip` disabled | Tooltip explains page is unavailable |

The button uses `aria-pressed="true"` only in selection, drawing, and
confirmation modes. Saving uses `aria-busy="true"` on the reader region.

## 6. Keyboard contract

### KEY-001: Enter Clip mode

Unmodified `C` toggles browse ↔ selection when all conditions are true:

- The reader is open.
- No `Ctrl`, `Meta`, or `Alt` modifier is held.
- The event is not an auto-repeat.
- The target is not an input, textarea, select, contenteditable element, button
  with text editing behavior, or editor-owned node.
- A completed page exists.
- A save is not in progress.

`Shift+C` may also activate because the key normalizes to `c`, but the UI
advertises only `C`.

### KEY-002: Escape hierarchy

`Escape` is resolved in this order:

1. **Saving:** consumed; saving is not cancelled after canonical creation has
   begun. The reader announces `Clipping is still saving`.
2. **Confirm:** discard the draft selection and return to browse mode.
3. **Drawing:** cancel the active drag, release pointer capture, and return to
   browse mode.
4. **Selection, no drag:** exit to browse mode.
5. **Browse:** preserve existing reader behavior and close/return after flushing
   reading progress.

One `Escape` press performs one transition. It never both cancels a selection
and closes the reader.

### KEY-003: Existing page navigation

- Left/right arrows retain page navigation in browse and selection-without-drag
  states.
- They are ignored in drawing, confirmation, and saving states.
- Changing pages in selection-without-drag keeps Clip mode active so the user
  can choose another page.
- Existing focusable controls keep native keyboard behavior and do not trigger
  reader page shortcuts.

### KEY-004: Focus after actions

- Entering Clip mode moves programmatic focus to the reader canvas only when
  activation came from the toolbar; it does not steal focus after keyboard
  activation.
- Entering confirmation focuses **Save clipping**.
- Choosing **Redraw** focuses the reader canvas and stays in selection mode.
- Choosing **Cancel** focuses the Clip toolbar action.
- Successful save returns focus to the Clip toolbar action unless the user
  activates the toast’s **Open note** action.

## 7. Pointer and selection behavior

The detailed gesture implementation is in specification 04. The user-visible
contract is:

### SELECT-001: Valid starting target

A drag starts only on the pixels of a completed newspaper page image. It does
not start on:

- Page margins outside the image.
- Reader toolbar or pagination controls.
- Failed/unavailable page placeholders.
- Selection confirmation controls.
- Scrollbars.

### SELECT-002: Direction independence

The user may drag in any direction. The displayed and saved rectangle is the
minimum/maximum box of start and current points.

### SELECT-003: Page boundary

The rectangle is clamped to the selected page image. Leaving the image while
holding the pointer extends the rectangle only to that image’s edge. A single
selection never crosses into another virtual page.

### SELECT-004: Scroll and zoom

- Before pointer-down, wheel/trackpad scrolling remains available so the user
  can choose a page while Clip mode is active.
- Once drawing begins, reader scroll and zoom are temporarily frozen until
  pointer-up, pointer-cancel, or Escape.
- Zoom, page dropdown, previous/next buttons, tone controls, and click-zoom are
  disabled during confirmation and saving.
- Browser/window resize preserves the selection because the overlay uses
  normalized geometry.

### SELECT-005: Visual treatment

During drawing and confirmation:

- The selected rectangle has a 2 px high-contrast border.
- The unselected part of that page is subdued with a translucent overlay.
- The selected pixels are not filtered or dimmed.
- Handles are not required in V1; **Redraw** starts a new selection.
- The overlay never becomes part of canonical capture.
- Reduced-motion mode removes animated transitions but preserves state changes.

### SELECT-006: Minimum region

The frontend estimates output dimensions from the manifest and blocks obvious
regions below 32×32 source pixels. Rust is authoritative. If backend validation
still rejects the region, confirmation remains open and shows:

```text
Select a larger area
The saved region must be at least 32 × 32 source pixels.
```

### SELECT-007: Page changes during selection

- Scrolling or arrow navigation before a drag changes the active page and keeps
  selection mode active.
- A page change after a drag has begun cancels the draft selection and returns
  to selection mode on the new page.
- Confirmation controls disable page changes, so a confirmed rectangle never
  silently rebinds to another page.

## 8. Confirmation surface

### UI-CONFIRM-001

The confirmation surface is anchored near the selected rectangle when space
allows and otherwise inside the reader viewport. It must not cover the entire
selection.

It contains:

```text
Save clipping
Redraw
Cancel
```

Optional non-authoritative information:

```text
Approximately 1,240 × 620 px
```

The approximate dimensions are explicitly labeled approximate because Rust
uses decoded source dimensions and authoritative rounding.

### UI-CONFIRM-002: Actions

- **Save clipping:** submits exactly one request, disables all confirmation
  actions, and enters saving state.
- **Redraw:** discards the current rectangle and returns to selection mode on
  the same page.
- **Cancel:** discards the rectangle and returns to browse mode.
- Double-clicking **Save clipping** or pressing Enter repeatedly cannot create
  duplicate records; the frontend prevents duplicate submission and the
  backend operation has one generated clipping ID per accepted invocation.

## 9. Save results and exact messages

### SAVE-001: Success

Title:

```text
Clipping saved
```

Description format:

```text
<Edition name> · <localized long date> · Page <page number>
```

Action:

```text
Open note
```

Toast duration must be long enough to act on; use the existing application
standard for actionable toasts and do not auto-open the note.

### SAVE-002: Stale media

Inline message in confirmation:

```text
The page image changed while you were saving.
Refresh the page and try again.
```

Actions:

```text
Refresh and retry
Cancel
```

`Refresh and retry` reloads the reader manifest, verifies the page still exists,
keeps the normalized rectangle when dimensions remain compatible, updates the
expected media version, and requires the user to press **Save clipping** again.
It does not silently submit a second save.

### SAVE-003: Source unavailable

```text
This newspaper page is no longer available.
The clipping was not created.
```

Action:

```text
Close
```

The reader returns to browse mode and refreshes the page state.

### SAVE-004: Busy crop service

Because V1 allows one concurrent crop operation, another request may wait. The
current reader shows `Waiting to save…` after 300 ms and `Saving…` once work
starts. A request is not failed merely because it waited in the bounded queue.

If the backend rejects because the service is shutting down:

```text
Clipping could not be saved
LinkVault is closing. Try again after reopening the app.
```

### SAVE-005: Generic recoverable failure

```text
Clipping could not be saved
No clipping or note was created. Try again.
```

The technical error code may be included in a copyable diagnostics area, but
absolute paths and raw database errors are never shown.

### SAVE-006: Ambiguous completion

If the frontend loses the command response after the backend may have committed,
it must not immediately resubmit. It queries by the operation/clipping ID first.

User copy while resolving:

```text
Checking whether the clipping was saved…
```

Resolution is either normal success or a confirmed not-created failure.

## 10. Clippings view product contract

Detailed data and editor behavior are in specification 05.

### VIEW-001: Empty state

Heading:

```text
No clippings yet
```

Body:

```text
Open a downloaded newspaper and choose Clip in the reader toolbar to save a
section with your notes.
```

Primary action:

```text
Open newspaper library
```

### VIEW-002: Master-detail layout

Desktop baseline:

```text
┌─────────────────────────────────────────────────────────────────────┐
│ Clippings            Search…                   Recently updated     │
├────────────────────────────┬────────────────────────────────────────┤
│ thumbnail                  │ source clipping image                  │
│ title                      │ edition · date · page                  │
│ provenance · updated       │ Open source                            │
│ note excerpt               ├────────────────────────────────────────┤
│                            │ editable title                         │
│ next clipping…             │ Markdown WYSIWYG editor               │
│                            │ Saving… / Saved / Failed / Conflict    │
└────────────────────────────┴────────────────────────────────────────┘
```

At narrow desktop widths, the view may switch to list → detail navigation, but
all actions and save guarantees remain the same.

### VIEW-003: List row

Each loaded row includes:

- Aspect-preserving thumbnail or deterministic placeholder.
- Title.
- Edition name.
- Publication date.
- Page number.
- Plain-text note excerpt when present.
- Last updated timestamp.
- Source-unavailable indicator only when applicable.

The row does not display raw asset paths, checksums, database IDs, or note
Markdown syntax.

### VIEW-004: Detail source card

The fixed source card includes:

- Canonical clipping image or missing-asset warning.
- Edition, date, and page provenance snapshots.
- `Open source` when source links still resolve.
- Disabled `Source unavailable` explanation when they do not.
- An optional click-to-expand read-only image overlay.
- No crop, annotation, replace-image, or delete-image control inside the card.

### VIEW-005: Editor entry

Opening a clipping focuses the note body only when navigation came from the
reader’s **Open note** action. Ordinary list selection does not steal focus.

The empty editor placeholder is:

```text
Add your notes…
```

The image remains visible above the first editable paragraph.

## 11. Search and sorting contract

### SEARCH-001

The search field placeholder is:

```text
Search titles, notes, editions, dates, or pages
```

Behavior:

- Input debounces 200 ms.
- Search is local and case-insensitive where SQLite supports case folding.
- Chinese and punctuation searches match literal substrings.
- `%`, `_`, and the chosen SQL escape character are treated literally.
- Search does not block note typing or autosave.
- Clearing search restores the previous default sort and list position only
  when the selected clipping remains in the result set; otherwise it selects
  the first result.

### SORT-001

V1 sort options:

```text
Recently updated
Newest clipping
Publication date
Title A–Z
```

Default: `Recently updated`.

Sort does not change the selected clipping if it remains present.

## 12. Rename and note behavior

### NOTE-001: Title

- Editable in the detail pane.
- Trimmed on save.
- 1–200 Unicode scalar values.
- Empty after trimming is rejected inline and the last valid saved title
  remains authoritative.
- Updating only the title increments clipping revision and updated time.

### NOTE-002: Markdown body

- May be empty.
- Maximum 2 MiB UTF-8.
- NUL characters are rejected.
- Supported formatting is defined by D-025.
- Source image is not part of the Markdown string.

### NOTE-003: Save indicators

Exact states:

| State | Label | Meaning |
|---|---|---|
| Clean | `Saved` | UI matches persisted revision |
| Dirty | `Unsaved changes` | Local value differs; debounce pending |
| Saving | `Saving…` | One update request in flight |
| Failed | `Save failed` and `Retry` | Local draft remains intact |
| Conflict | `Changed elsewhere` | Local draft remains; user must choose resolution |

The UI never shows `Saved` before the matching backend response is received.

## 13. Open-source behavior

### SOURCE-001: Available

Choosing **Open source**:

1. Flushes pending note changes.
2. Navigates to the matching newspaper edition and exact page.
3. Preserves a return target to the clipping.
4. Scrolls the source-pixel rectangle into view.
5. Shows a non-interactive highlight for three seconds or until the user
   scrolls, zooms, enters Clip mode, or changes page.
6. Labels the reader back action `Back to clipping`.

The transient highlight is not a new selection and cannot be saved again
without explicitly entering Clip mode.

### SOURCE-002: Unavailable

The source card shows:

```text
Original edition is no longer in the newspaper library.
Your clipping and note are still saved.
```

`Open source` is disabled. The image and note remain fully usable.

## 14. Delete interaction

### DELETE-001: Entry point

Delete is available from the detail overflow menu and an accessible list-row
context action. It is not placed next to ordinary editor formatting controls.

### DELETE-002: Confirmation

Title:

```text
Delete this clipping?
```

Body:

```text
This removes the saved image and its note from LinkVault. The original
newspaper page is not deleted.
```

Actions:

```text
Delete clipping
Cancel
```

The destructive action uses the application’s danger styling and requires a
new explicit click; Enter on opening the dialog must not immediately confirm.

### DELETE-003: Result

- On success, the row disappears and the next logical item is selected.
- On failure, the clipping remains selected, local note text remains intact,
  and the UI shows a retryable error.
- Closing the app during deletion must recover to either a readable clipping or
  a completed deletion; the user is not left with a silently broken row.

## 15. Reset interaction

The World Journal reset dialog must state:

```text
Resetting World Journal removes downloaded-edition records, reading progress,
schedules, and generated newspaper previews.

Your saved clippings and clipping notes are preserved.
```

The success message must not imply that all World Journal data was deleted. A
clipping-specific destructive reset does not exist in V1.

## 16. Accessibility requirements

### A11Y-001: Mode announcement

A polite live region announces entry, cancellation, confirmation, save start,
success, and recoverable errors. Repeated pointer movements do not spam the live
region.

### A11Y-002: Non-color indicators

Selection, errors, source-unavailable state, dirty state, and conflicts use
text/icon/border cues in addition to color.

### A11Y-003: Focus visibility

Every action has a visible keyboard focus indicator in light and dark themes.
The selection rectangle does not replace focus indication.

### A11Y-004: Contrast

Selection border, outside-selection mask, toolbar active state, disabled state,
toasts, save indicators, and warning surfaces meet WCAG AA contrast against the
reader tones and application themes.

### A11Y-005: Motion

`prefers-reduced-motion` removes selection-control motion, toast movement where
supported by existing primitives, and transient highlight animation. The
three-second highlight may remain static and disappear without fading.

### A11Y-006: Screen-reader labels

Required labels include:

- `Clip newspaper page`
- `Save clipping`
- `Redraw clipping selection`
- `Cancel clipping selection`
- `Search clippings`
- `Sort clippings`
- `Open source newspaper page`
- `Delete clipping`
- `Clipping note title`
- `Clipping note editor`

Thumbnail alt text is concise provenance, for example:

```text
Clipping from New York, August 7, 2026, page A06
```

The full image and thumbnail are not both announced when shown together in the
same detail context.

## 17. Responsive and native-window behavior

- Baseline minimum supported content width for two panes is 900 CSS px.
- Below that width, the list becomes a full-width page and selection opens a
  full-width detail view with an explicit `Back to clippings` action.
- Reader selection remains correct at Windows display scaling 100%, 125%, 150%,
  and 200%.
- Multi-monitor movement during an inactive selection is allowed; movement
  during an active pointer drag cancels the drag safely if the browser emits
  pointer cancellation.
- Window minimize/blur during drawing cancels the draft and returns to browse
  mode. During saving it continues and resolves on restore.

## 18. Edge cases

### EC-PRODUCT-001: No completed page

Clip is disabled and explains `This page is not available to clip.`

### EC-PRODUCT-002: Optimization changes media

Save returns stale-media behavior; no clipping row is created from an
unacknowledged version.

### EC-PRODUCT-003: Source disappears after save

The clipping opens normally; source link changes to unavailable on refresh.

### EC-PRODUCT-004: Canonical image missing

The detail pane preserves title, note, and provenance and shows:

```text
Saved image is unavailable
Your note is still safe. LinkVault could not find or validate the clipping
image.
```

Recovery actions, when available, are specified in 06. The row is never
silently deleted.

### EC-PRODUCT-005: Empty note

A clipping with no note text is valid and appears in the library.

### EC-PRODUCT-006: Rapid multi-save

The user may save multiple regions sequentially. Each success creates a unique
clipping. One crop operation executes at a time, and later requests show a
waiting state rather than duplicating or dropping work.

### EC-PRODUCT-007: Duplicate region

V1 does not deduplicate identical regions. Saving the same rectangle twice
creates two independent clipping notes.

### EC-PRODUCT-008: Note update conflict

The editor keeps the local draft visible and presents the conflict-resolution
contract from specification 05. It never silently reloads or overwrites.

### EC-PRODUCT-009: Search excludes selected clipping

The detail pane closes or selects the first result only after dirty content is
flushed. It never discards an unsaved draft because a debounced search response
arrived.

### EC-PRODUCT-010: App close during debounce

The cooperative close handler attempts an immediate flush and may delay normal
close briefly. Forced process termination or power loss inside the debounce
window remains a documented limitation.

## 19. Final product acceptance criteria

### AC-PRODUCT-002: Non-disruptive capture

Given a user reading page A06

When they save a valid clipping

Then the reader returns to page A06 at the previous zoom and scroll position

And the user can save another clipping without reopening the edition.

### AC-PRODUCT-003: Durable note

Given a saved clipping with title and Markdown note

When LinkVault restarts

Then the title, note, canonical image, provenance, and revision reload
unchanged.

### AC-PRODUCT-004: Reset preservation

Given saved clippings whose source editions exist

When World Journal reset completes

Then the clippings and notes remain searchable and editable

And every source link correctly reports unavailable.

### AC-PRODUCT-005: Exact source return

Given a clipping whose source exists

When the user chooses Open source

Then the reader opens the correct edition and page

And the saved rectangle is transiently highlighted at the correct normalized
location

And Back returns to the same clipping.

### AC-PRODUCT-006: Accessible operation

Given keyboard-only or screen-reader use

When the user enters Clip mode, cancels, saves, opens a note, edits, and deletes

Then every state and required action is reachable, labeled, and announced
without relying only on color or pointer hover.

### AC-PRODUCT-007: Clear integrity states

Given one clipping with a missing source and another with a missing canonical
asset

When both are opened

Then the first remains normally readable with Open source disabled

And the second shows an asset-integrity warning while preserving its note and
provenance.
