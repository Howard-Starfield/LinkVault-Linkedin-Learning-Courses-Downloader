# Newspaper Clippings V1: reader selection interface

**Status:** Approved

**Primary implementation phase:** Phase 3

**Related decisions:** D-001 through D-006, D-013, D-022, D-030

## 1. Purpose

This specification defines how clipping capture integrates into the existing
virtualized `NewspaperReader` without breaking click-to-zoom, drag-to-pan,
keyboard page navigation, reading-progress persistence, reader tones, or the
three-page mounted-image performance contract.

The current reader already owns pointer-down/move/up/cancel for zoom and pan,
uses `Escape` to close, uses left/right arrows for page navigation, and renders
only a bounded virtual page range. Clipping must be an explicit interaction
state with higher priority than those existing gestures; it must not be layered
on as an independent competing handler.

## 2. Phase visibility rule

Phase 3 implements and tests the complete reader capture workflow against the
Phase 2 backend, but production navigation must not expose an incomplete
feature before the Clippings view exists.

### FR-READER-001

Until Phase 4B adds the Clippings view and final `Open note` navigation, the
reader capture action is enabled only in the dedicated test harness or behind a
LinkVault-owned internal capability prop whose production value remains false.

Conceptual prop:

```ts
export type NewspaperReaderProps = {
  // existing props...
  clippingCapability?: {
    enabled: boolean;
    onCreated?: (clippingId: string) => void;
  };
};
```

Phase 4B removes the temporary disabled production wiring or turns the final
capability on through normal application composition. No user-facing setting
or permanent experimental flag is created.

## 3. Component boundaries

Recommended frontend ownership:

```text
apps/desktop/src/components/newspaper/
├─ NewspaperReader.tsx
├─ newspaper-api.ts
├─ newspaper-clipping-geometry.ts
├─ NewspaperClippingSelectionOverlay.tsx
└─ newspaper-reader-clipping.test/support files
```

Exact file split may vary, but pure geometry and editable-target detection must
not remain buried inside the large reader component.

### FR-READER-002

`newspaper-clipping-geometry.ts` contains no React, Tauri, DOM mutation, timers,
or global state. It exposes deterministic helpers for:

- Clamping client points to an image rectangle.
- Converting client points to normalized coordinates.
- Normalizing reverse-direction drags.
- Estimating source-pixel dimensions from manifest dimensions.
- Rejecting non-finite or zero-area selections.

### FR-READER-003

The overlay component receives normalized geometry and state. It does not own
backend invocation, page navigation, or persistent note state.

## 4. Reader interaction state machine

Binding TypeScript model:

```ts
type ReaderBrowseState = {
  type: "browse";
};

type ReaderClipSelectingState = {
  type: "clip-selecting";
};

type ReaderClipDrawingState = {
  type: "clip-drawing";
  pointerId: number;
  pageId: string;
  pageIndex: number;
  expectedMediaVersion: number;
  image: HTMLImageElement;
  imageRectAtStart: DOMRectReadOnly;
  startClientX: number;
  startClientY: number;
  currentClientX: number;
  currentClientY: number;
};

type ReaderClipConfirmingState = {
  type: "clip-confirming";
  pageId: string;
  pageIndex: number;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
  estimatedWidth: number | null;
  estimatedHeight: number | null;
  errorCode?: string | null;
};

type ReaderClipSavingState = {
  type: "clip-saving";
  operationId: string;
  pageId: string;
  pageIndex: number;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
  queueState: "pending" | "running";
};

type ReaderInteractionState =
  | ReaderBrowseState
  | ReaderClipSelectingState
  | ReaderClipDrawingState
  | ReaderClipConfirmingState
  | ReaderClipSavingState;
```

A reducer is preferred so every event has one reviewed transition. Equivalent
state organization is acceptable only if transition tests remain exhaustive.

## 5. Event priority

The reader resolves an input event in this priority:

```text
clipping save lock
→ clipping confirmation controls
→ clipping drag
→ clipping selection waiting for pointer-down
→ reader pan
→ reader click zoom
→ ordinary page navigation
```

### FR-READER-004

Existing pan/click-zoom handlers return before mutating gesture state whenever
interaction state is not `browse`.

### FR-READER-005

Clipping handlers never reuse `panGestureRef` as a loosely typed second mode.
They use their own explicit state/refs so a pointer cannot simultaneously be a
pan and a clipping drag.

## 6. Entering and leaving Clip mode

### Enter through toolbar

From `browse`:

1. Verify capability enabled and active completed page exists.
2. Clear any transient source-highlight overlay from Phase 5.
3. Cancel an incomplete pan gesture and release pointer capture safely.
4. Transition to `clip-selecting`.
5. Set Clip button pressed state.
6. Focus the reader canvas.
7. Announce instructions once.

### Enter through keyboard

The global reader key handler uses:

```ts
function isEditableKeyboardTarget(target: EventTarget | null): boolean
```

It returns true for:

- `INPUT`, `TEXTAREA`, `SELECT`.
- Any element with `contenteditable` not equal to `false`.
- Any ancestor marked `data-editor-root="true"`.
- Any future adapter-provided editable marker.

Unmodified, non-repeating `C` from a non-editable target transitions to
`clip-selecting` without changing focus.

### Leave selection mode

From `clip-selecting`, pressing Clip again, `C`, or `Escape` returns to browse.
No backend request or file is created.

### FR-READER-006

Entering/leaving Clip mode preserves:

- Current active page.
- Reader zoom.
- Click-zoomed zoom level and restore target.
- Horizontal/vertical scroll position.
- Page tone.
- Reading-progress timers and latest saved progress.

It disables click zoom and pan only while clipping state is active; it does not
reset those preferences.

## 7. Page image metadata

Each completed page image/wrapper supplies stable attributes needed by the
selection handler:

```text
data-page-id
ndata-page-index (or equivalent typed lookup; exact attribute must be `data-page-index`)
data-media-version
data-source-width
data-source-height
data-testid="newspaper-reader-page-image"
```

The accidental `ndata` spelling above is not binding; the actual required
attribute is:

```text
data-page-index
```

### FR-READER-007

Metadata values come from the current reader manifest. The handler does not
parse page ID, version, or dimensions from media URLs.

### FR-READER-008

The active target must be a connected `HTMLImageElement` inside a completed
`.newspaper-reader-page`. Failed placeholders and loading images are ineligible.

### FR-READER-009

A page image must be fully decoded enough to have a positive displayed bounding
rectangle. Pointer-down on a zero-size/loading image is ignored with no state
change.

## 8. Pointer-down and drag

### FR-POINTER-001: Start

In `clip-selecting`, primary-button pointer-down on an eligible image:

1. Resolve the image and page metadata.
2. Read its current `getBoundingClientRect()`.
3. Clamp the starting client point to the rectangle.
4. Store page identity, media version, image reference, rectangle, and pointer
   ID in `clip-drawing`.
5. Capture the pointer on the stable reader interaction surface.
6. Freeze scroll/zoom/page controls for the duration of drawing.
7. Prevent the event from becoming click zoom or pan.

Non-primary pointers, non-left mouse buttons, and pointer-down outside a page
remain ignored.

### FR-POINTER-002: Move

For the captured pointer:

- Read the image’s current bounding rectangle, not only the startup rectangle,
  if layout may have changed.
- Clamp the current client point to that rectangle.
- Convert start/current into normalized min/max bounds.
- Render percentages relative to the image.
- Do not emit backend requests or React state updates faster than one animation
  frame. High-frequency pointer events may update refs and schedule one render.

### FR-POINTER-003: Layout movement

Drawing freezes reader-controlled scroll and zoom, but the window may still
move or resize. If the image rectangle changes, recompute normalized geometry
using the current rectangle and the stored client points. If the image becomes
disconnected or has zero size, cancel safely to `browse` and announce that the
selection was cancelled.

### FR-POINTER-004: Leave image

Pointer movement outside the page is clamped to the page edge. It never selects
reader margins, another page, toolbar, or browser content.

### FR-POINTER-005: Reverse direction

The overlay uses:

```text
left = min(startX, currentX)
top = min(startY, currentY)
right = max(startX, currentX)
bottom = max(startY, currentY)
```

before normalization. All four drag directions produce equivalent geometry.

## 9. Pointer completion and cancellation

### FR-POINTER-006: Pointer-up

On matching pointer-up:

1. Compute final normalized rectangle.
2. Release pointer capture.
3. Restore frozen scroll behavior.
4. If zero-area or estimated below 32×32 source pixels, return to
   `clip-selecting`, retain no rectangle, and announce `Select a larger area`.
5. Otherwise transition to `clip-confirming` and focus **Save clipping**.

A clipping drag has no pan threshold; any region must pass source-size
validation. The existing pan drag threshold remains browse-only.

### FR-POINTER-007: Pointer-cancel/lost capture

`pointercancel`, `lostpointercapture`, window blur during drawing, or component
unmount:

- Releases any remaining capture when possible.
- Restores scroll behavior.
- Clears drawing refs.
- Returns to browse.
- Does not invoke create.

### FR-POINTER-008: Wheel/touch/scroll lock

During drawing, confirmation, and saving, the reader must prevent wheel,
trackpad, touch-scroll, scrollbar drag, page-button, page-select, and zoom-control
changes from moving the selected page. Use a non-passive native wheel handler or
an equally reliable browser mechanism only while those states are active.

In `clip-selecting` before drawing, ordinary wheel/trackpad scroll remains
available.

## 10. Selection overlay geometry

The overlay is rendered inside a `position: relative` page-image wrapper and
uses normalized percentages:

```text
left:   rect.x * 100%
top:    rect.y * 100%
width:  rect.width * 100%
height: rect.height * 100%
```

### FR-OVERLAY-001

Do not render a viewport-sized canvas or copy the page image into a canvas for
selection. The native `<img>` remains the visual source.

### FR-OVERLAY-002

Outside dimming may use four absolutely positioned mask rectangles or an
outline/box-shadow technique. It must be clipped to the page image and must not
apply a CSS filter to the selected pixels.

### FR-OVERLAY-003

The selection rectangle has:

- A 2 CSS px border.
- A visible treatment against original, soft, dim, and inverted page tones.
- No resize handles in V1.
- `pointer-events: none` except the separate confirmation controls.

### FR-OVERLAY-004

The overlay has `aria-hidden="true"`. Instructions and state are announced by a
separate live region because the geometric rectangle has no useful screen-reader
content.

## 11. Confirmation state

### FR-CONFIRM-001

Confirmation stores immutable page ID, page index, expected media version, and
normalized rectangle. It does not recompute against a different active page.

### FR-CONFIRM-002

Controls:

- Save clipping — primary.
- Redraw — secondary.
- Cancel — ghost/secondary.

All other reader controls that could change page geometry are disabled.

### FR-CONFIRM-003

Approximate dimensions use current manifest `pixelWidth`/`pixelHeight` and the
same floor/ceil algorithm as the backend where possible. The UI labels them
`Approximately` and never treats them as authoritative.

### FR-CONFIRM-004

`Redraw` returns to `clip-selecting` on the same page and clears any previous
error. `Cancel` returns to browse. Neither creates an operation ID.

## 12. Saving state

### FR-SAVE-READER-001: Operation creation

Generate `operationId` with `crypto.randomUUID()` exactly once when the user
first activates **Save clipping** for the confirmed rectangle. Retain it for
transport retry or ambiguous-completion lookup. Redrawing or creating a new
explicit clipping generates a new ID.

### FR-SAVE-READER-002: Duplicate guard

Before invoking:

- Transition synchronously to `clip-saving`.
- Disable confirmation and toolbar controls.
- Store one in-flight promise/ref keyed by operation ID.
- Ignore duplicate click/Enter events while it exists.

### FR-SAVE-READER-003: Queue label

Start a 300 ms timer:

- Before 300 ms: `Saving…` is acceptable.
- If backend progress distinguishes waiting from running, show
  `Waiting to save…` while queued and `Saving…` while active.
- Do not create polling faster than existing application standards solely for
  this label.

### FR-SAVE-READER-004: Success

On success:

1. Clear save timers and in-flight ref.
2. Return to browse.
3. Preserve active page, zoom, and scroll position.
4. Emit `onCreated(clippingId)` to application composition.
5. Show the exact success toast from specification 01.
6. In the final Phase 4B composition, the toast action navigates to the clipping.

Phase 3’s hidden test harness may assert the callback without exposing the
production action.

### FR-SAVE-READER-005: Typed failure

Failures map by code:

- `SOURCE_MEDIA_STALE`: remain in confirmation, retain rectangle, offer
  refresh-and-retry.
- `CROP_TOO_SMALL`: remain in confirmation and instruct larger redraw.
- Source not found/not ready/unavailable: exit to browse after acknowledgment
  and refresh manifest.
- Encode/storage/database retryable failure: remain in confirmation with Retry
  using the same operation ID only when backend idempotency says no duplicate
  can be created.
- Invalid/security failure: exit to browse after safe error; no blind retry.

The local selection remains available for retry whenever doing so cannot bind
it to a different page/version silently.

### FR-SAVE-READER-006: Component close while saving

The reader back action and browse-closing `Escape` are disabled/consumed during
saving. App shutdown may wait through the backend’s recoverable boundary as
specified in 03. The component must not unmount and discard response ownership
because the user clicked Back repeatedly.

## 13. Stale-media refresh

### FR-STALE-001

Refresh-and-retry performs:

1. Fetch current reader manifest for the job.
2. Find the same page ID.
3. Require status completed.
4. Update page data and expected media version.
5. Keep normalized rectangle because it is page-relative.
6. Show confirmation again with a message that the page refreshed.
7. Require a new explicit **Save clipping** activation.

A new operation ID is generated because the prior request was confirmed not to
have created a row. If backend state for the old ID is ambiguous, resolve it
first.

### FR-STALE-002

If the page ID no longer exists or is incomplete, discard confirmation and show
source unavailable. Do not redirect the rectangle to the page now occupying the
same index.

## 14. Keyboard state handling

The existing global keydown effect must be updated in one reviewed handler or
composed handlers with explicit priority.

### FR-KEY-READER-001

State matrix:

| Key | Browse | Clip selecting | Drawing | Confirming | Saving |
|---|---|---|---|---|---|
| `C` | Enter Clip | Exit Clip | Ignore | Ignore | Ignore |
| `Escape` | Existing close | Exit to browse | Cancel to browse | Cancel to browse | Consume/announce |
| Left/Right | Navigate | Navigate, remain Clip | Ignore | Ignore | Ignore |
| Enter | Native focused action | Native | Ignore | Activates focused button | Ignore |
| Tab | Native reader controls | Native controls | Ignore until cancel/up | Cycles confirm controls | Focus remains within safe reader controls |

### FR-KEY-READER-002

The key handler first ignores events whose default was already prevented by an
editable/control context. It must not prevent copy, text editing, select
keyboard operation, or future note-editor shortcuts.

## 15. Reader virtualization preservation

### FR-VIRTUAL-001

The existing virtualizer remains authoritative for mounted page ranges. Clip
mode must not render hidden duplicate `<img>` elements, offscreen canvases, or
all page images.

### FR-VIRTUAL-002

The existing `data-mounted-page-images` metric remains accurate and at most
three page media images are mounted under normal reader operation.

### FR-VIRTUAL-003

During drawing/confirmation/save, the target page is already in the current
three-page range. Scroll lock prevents it from leaving. No pinning of additional
pages is needed.

### FR-VIRTUAL-004

After saving/canceling, virtualizer measurements and active-page tracking resume
without a forced full reader remount or scroll-to-index jump.

## 16. Reading-progress behavior

### FR-PROGRESS-001

Entering Clip mode does not mark a different page viewed. Existing active page
progress debounce continues normally.

### FR-PROGRESS-002

Saving a clipping does not write reading progress inside the crop transaction or
change furthest-page semantics.

### FR-PROGRESS-003

Closing from browse after a clipping operation still flushes progress through
the existing retry path. Clip errors do not suppress progress saving.

## 17. Page tones and appearance

### FR-TONE-001

Original, soft, dim, and inverted tones remain visual reader preferences. The
selection overlay stays legible in every tone.

### FR-TONE-002

Tone controls are available in `clip-selecting` before drawing if they do not
change page geometry. They are disabled in drawing, confirmation, and saving.

### FR-TONE-003

Tone is never included in the create request. Frontend tests assert identical
requests for equivalent geometry under all tones.

## 18. Browser test harness

Phase 3 adds a deterministic browser harness with:

- Fixed reader manifest of at least five pages.
- Generated page images with known dimensions and visible grid markers.
- Mocked `create_newspaper_clipping` invoke capturing exact request payloads.
- Configurable delayed, stale, success, and failure responses.
- Capability enabled only in harness.
- Existing reader zoom/pan/virtualization instrumentation preserved.

Required test IDs/data attributes may include:

```text
data-testid="newspaper-reader-clip"
data-testid="newspaper-clipping-selection"
data-testid="newspaper-clipping-confirm"
data-testid="newspaper-clipping-save"
data-testid="newspaper-clipping-redraw"
data-testid="newspaper-clipping-cancel"
data-clipping-mode="browse|selecting|drawing|confirming|saving"
```

Test hooks must remain semantic and not expose implementation-only raw paths.

## 19. Automated interaction matrix

### AC-READER-001: Toolbar flow

Click Clip → drag → Save sends exactly one request with page ID, current media
version, operation ID, and expected normalized rectangle.

### AC-READER-002: Shortcut safety

Unmodified C toggles from canvas, while C inside input/contenteditable/editor,
Ctrl/Cmd+C, Alt+C, and repeat events do not toggle.

### AC-READER-003: Escape priority

Each state follows the table in section 14, and cancelling a selection never
also closes the reader.

### AC-READER-004: Pan/zoom isolation

Given pan-enabled zoom

When Clip mode is active and a drag occurs

Then scroll position does not change from pan

And click zoom does not toggle

And leaving Clip mode restores ordinary pan/click behavior.

### AC-READER-005: Geometry invariance

Equivalent source regions selected at 50%, 100%, 120%, and 300% reader zoom send
normalized coordinates equal within the declared frontend tolerance.

### AC-READER-006: Reverse directions

All four drag directions produce the same normalized rectangle.

### AC-READER-007: Page clamp

Dragging beyond all image edges yields exactly 0/1-clamped bounds and never
includes adjacent page/margin pixels.

### AC-READER-008: Resize

Resizing between pointer-down and pointer-up either preserves correct normalized
geometry or cancels safely if the image becomes invalid; it never saves stale
client-pixel geometry.

### AC-READER-009: Virtualization

Across 8, 50, and 500-page manifests and all clipping states,
`data-mounted-page-images <= 3` and no hidden duplicate source image is created.

### AC-READER-010: Duplicate submit

Double-click and repeated Enter on Save produce one invoke and one operation ID.

### AC-READER-011: Stale retry

Stale response retains rectangle, refreshes the same page ID, updates version,
requires explicit save, and never redirects by page index.

### AC-READER-012: Save position

Success returns to browse with the same active page, zoom, horizontal scroll,
and vertical scroll within a one-CSS-pixel browser tolerance.

### AC-READER-013: Failure cleanup

Every pointer cancellation, blur, save error, reader unmount, and disabled
capability path releases capture/timers/listeners and leaves no stuck scroll
lock or pressed state.

### AC-READER-014: Tone independence

Equivalent selections in all page tones send identical create payloads.

### AC-READER-015: Accessibility

Clip mode, confirmation, save, cancellation, and errors are keyboard reachable,
focus-visible, and announced once without pointer-move spam.

## 20. Native Windows manual matrix

Phase 3 exit requires a Tauri native smoke, not only browser automation.

Test at display scaling:

```text
100%
125%
150%
200%
```

For each scale:

- Reader zoom 50%, 100%, 120%, 300%.
- Original and inverted tones at minimum; all tones in final Phase 6.
- Normal window and maximized.
- Horizontal pan at zoom > baseline.
- First, middle, and last page.
- Top-left and bottom-right selections.
- Reverse drag.
- Escape during drawing and confirmation.
- Save success and stale failure fixture where practical.
- Move window between monitors with different scaling before starting a drag.

Evidence records screenshots or screen capture of overlay alignment and the
captured normalized request—not canonical screenshot pixels.

## 21. Phase 3 exit gate

Phase 3 is complete only when:

- Phase 2 is merged and green.
- Production capability remains unexposed until Phase 4B.
- Pure geometry and full browser interaction tests pass.
- Existing reader tests, three-image virtualization, zoom, pan, page navigation,
  tone, progress save, and close behavior remain green.
- Native Windows smoke passes at all four display scales.
- No new full-page canvas/screenshot path exists.
- No editor dependency or Clippings detail view is added.
- Existing UI, visual, Newspaper performance/browser performance, Rust,
  frontend build, and release gates remain green.
- The coding agent stops. Phase 4A/4B are separate reviews.
