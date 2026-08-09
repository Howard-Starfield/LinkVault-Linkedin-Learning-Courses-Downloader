# Newspaper Clippings V1: library and Markdown note editor

**Status:** Approved

**Primary implementation phases:** Phase 4A editor evaluation and Phase 4B
production integration

**Related decisions:** D-001, D-010 through D-015, D-018 through D-026, D-028,
D-029

## 1. Purpose

This specification defines the dedicated Clippings view, paged and virtualized
list, detail source card, search and sorting, frontend data contracts, internal
editor adapter, approved Markdown subset, autosave state machine, optimistic
conflict handling, navigation guards, empty/loading/error states, accessibility,
and editor-selection gate.

Phase 4A selects an editor behind a LinkVault-owned adapter. Phase 4B adds the
production view and enables the reader capture workflow only after the full
save-and-review path exists.

## 2. View ownership and file layout

Recommended frontend ownership:

```text
apps/desktop/src/components/newspaper/
├─ NewspaperClippings.tsx
├─ NewspaperClippingList.tsx
├─ NewspaperClippingDetail.tsx
├─ NewspaperClippingSourceCard.tsx
├─ ClippingNoteEditor.tsx
├─ clipping-note-save-controller.ts
├─ clipping-markdown.ts
└─ newspaper-api.ts
```

The exact split may vary, but these concerns must remain distinct:

- Paged list query and virtualization.
- Detail fetch and selected clipping identity.
- Canonical source-card presentation.
- Third-party editor adapter.
- Draft/autosave/revision state.
- App-level navigation target integration.

### FR-LIBRARY-001

`NewspaperView` or an equivalent provider composition boundary gains a
`clippings` mode/component. Clipping-specific state must not be added to the
existing download form or hidden inside `NewspaperLibrary` local state.

### FR-LIBRARY-002

Production application code outside `ClippingNoteEditor.tsx` does not import the
selected third-party editor package directly.

## 3. Tauri/frontend API contracts

All types use `camelCase` over IPC and return versioned media URLs, never paths.

### 3.1 List query

```ts
export type NewspaperClippingSort =
  | "updated_desc"
  | "created_desc"
  | "publication_desc"
  | "title_asc";

export type GetNewspaperClippingsPageRequest = {
  query: string;
  sort: NewspaperClippingSort;
  offset: number;
  limit: number;
};

export type NewspaperClippingSummary = {
  id: string;
  title: string;
  noteExcerpt: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  pageNumber: string;
  thumbnailReady: boolean;
  thumbnailUrl?: string | null;
  thumbnailVersion?: string | null;
  sourceAvailable: boolean;
  assetState: "ready" | "missing";
  assetErrorCode?: string | null;
  assetWidth: number;
  assetHeight: number;
  revision: number;
  createdAt: number;
  updatedAt: number;
};

export type NewspaperClippingPage = {
  items: NewspaperClippingSummary[];
  total: number;
  offset: number;
  limit: number;
  revision: number;
};
```

Command:

```text
get_newspaper_clippings_page
```

Validation and query semantics are defined in specification 02.

### 3.2 Detail query

```ts
export type NewspaperClippingDetail = NewspaperClippingSummary & {
  imageUrl?: string | null;
  imageVersion: number;
  noteMarkdown: string;
  sourceJobId?: string | null;
  sourcePageId?: string | null;
  sourceMediaVersion: number;
  sourceKind: "original" | "optimized";
  sourceMimeType: string;
  sourceWidth: number;
  sourceHeight: number;
  crop: {
    x: number;
    y: number;
    width: number;
    height: number;
    normalizedX: number;
    normalizedY: number;
    normalizedWidth: number;
    normalizedHeight: number;
  };
};
```

Command:

```text
get_newspaper_clipping
```

Input is one validated clipping ID.

### 3.3 Update command

```ts
export type UpdateNewspaperClippingRequest = {
  clippingId: string;
  expectedRevision: number;
  title: string;
  noteMarkdown: string;
};

export type UpdateNewspaperClippingResponse = {
  clippingId: string;
  title: string;
  noteMarkdown: string;
  revision: number;
  updatedAt: number;
};
```

Command:

```text
update_newspaper_clipping
```

### 3.4 Thumbnail ensure

```ts
export type EnsureNewspaperClippingThumbnailResult =
  | {
      status: "ready" | "generated";
      thumbnailUrl: string;
      thumbnailVersion: string;
      width: number;
      height: number;
    }
  | {
      status: "busy";
      retryAfterMs: number;
    }
  | {
      status: "unavailable";
      reason: "asset_missing" | "asset_invalid";
    };
```

Command:

```text
ensure_newspaper_clipping_thumbnail
```

### 3.5 Invalidation event

```text
newspaper://clippings-invalidated
```

Payload:

```ts
export type NewspaperClippingsInvalidatedEvent = {
  revision: number;
  clippingIds: string[];
  reason: "created" | "updated" | "deleted" | "recovered" | "source_changed";
};
```

`clippingIds` is bounded. A large reset/source-change event may use an empty
array to mean refresh affected pages rather than emit unbounded IDs.

## 4. Master-detail layout

### Desktop layout

At content width ≥900 CSS px:

```text
┌───────────────────────────────────────────────────────────────────────┐
│ Clippings     [Search…………………………] [Recently updated ▾]             │
├───────────────────────────────┬───────────────────────────────────────┤
│ virtualized list              │ selected clipping detail              │
│  thumbnail  title             │ fixed source card                     │
│             provenance        │ title field                           │
│             excerpt           │ WYSIWYG Markdown editor               │
│             updated           │ save state                            │
│                               │                                       │
└───────────────────────────────┴───────────────────────────────────────┘
```

Recommended starting pane sizes:

- Left: 340 CSS px, resizable only if an existing application split-pane
  primitive is available without adding a new dependency.
- Right: remaining width, minimum 480 CSS px.
- No second persistent right sidebar in V1.

At width <900 CSS px:

- List is full width.
- Selecting a clipping opens a full-width detail surface.
- Detail has `Back to clippings`.
- Dirty-navigation guards remain identical.

### FR-LAYOUT-001

Only the list pane scrolls its rows. Detail may have its own vertical scroll.
Scrolling a long note must not move the list position.

### FR-LAYOUT-002

Returning from detail or source reader restores list query, sort, selected ID,
and scroll position when still valid.

## 5. List loading and virtualization

The list follows the existing Newspaper library’s bounded loading pattern.

```text
PAGE_SIZE = 50
ESTIMATED_ROW_HEIGHT = 128 CSS px
OVERSCAN = 4 rows
SEARCH_DEBOUNCE = 200 ms
```

### FR-LIST-001: Sparse page model

The frontend may keep an array sized to `total` with undefined placeholders and
fetch page-aligned offsets as virtual rows enter the range. It uses a request
generation ID so stale search/sort responses cannot overwrite current results.

### FR-LIST-002: Deterministic keys

Loaded rows use clipping ID. Placeholders use stable index-based keys that
cannot collide with clipping IDs.

### FR-LIST-003: Prefetch

At most the current virtual page and one near-future page are prefetched. The
view must not eagerly fetch all 500 rows or all Markdown bodies.

### FR-LIST-004: Thumbnail requests

- Request thumbnails only for rows intersecting the visible viewport, not every
  overscan row, unless measurements prove an immediate one-row lookahead is
  beneficial.
- Coalesce in-flight requests by clipping ID.
- Honor backend `busy` retry delay.
- Cancel UI ownership on unmount/query generation change; backend cache work may
  finish safely.
- A failed thumbnail shows a deterministic placeholder and does not retry in a
  tight loop.

### FR-LIST-005: Row contents

Each row renders:

- Thumbnail with `object-fit: contain` in a fixed frame.
- Title, one or two lines with accessible full-title text.
- `<edition name> · <date> · <page>`.
- Plain-text note excerpt when non-empty.
- Relative or localized updated timestamp.
- Source-unavailable badge only when false.
- Asset-missing warning badge when `assetState = missing`.

No row renders hidden full Markdown, full canonical image, source paths, or
checksum.

### FR-LIST-006: Selection

- A deep target clipping ID is selected after detail validation.
- Without a deep target, the first loaded result is selected on desktop when
  total >0.
- On narrow layout, no automatic detail navigation occurs; the user chooses a
  row.
- If the selected clipping leaves the result set, pending edits are flushed or
  explicitly resolved before changing selection.
- After deletion, choose the row now at the deleted index, otherwise the prior
  row, otherwise empty state.

### FR-LIST-007: Refresh/invalidation

An invalidation event does not reset scroll to top. It refreshes the pages that
contain known affected IDs when practical; otherwise it reloads from page zero
while restoring scroll anchor/selected ID after layout.

## 6. Search and sorting

### Search

- Placeholder: `Search titles, notes, editions, dates, or pages`.
- Query capped and escaped as defined in specification 02.
- `Escape` in a non-empty focused search field clears it; a second Escape may
  follow normal application behavior.
- Empty query is represented as `""`, not `%` or null.
- Search state is local to the Clippings view for V1; it need not persist across
  app restart.

### Sort

Labels and values:

```text
Recently updated  → updated_desc
Newest clipping   → created_desc
Publication date  → publication_desc
Title A–Z         → title_asc
```

Sort selection may persist in localStorage under a versioned clipping-specific
key. It is preference data, not SQLite domain state.

### AC-LIBRARY-001

Given `%`, `_`, backslash/escape, apostrophe, Chinese text, and mixed date/page
queries

When search runs

Then values are treated literally, results are paged deterministically, and no
SQL syntax is interpolated.

## 7. Loading, empty, and failure states

### Initial loading

Show row skeletons and a detail skeleton without announcing `No clippings yet`
until the first list request resolves.

### Empty database

Exact copy from specification 01 with action `Open newspaper library`.

### Empty search result

```text
No matching clippings
Try a different title, note, edition, date, or page.
```

Action: `Clear search`.

### List failure

```text
Could not load clippings
Retry
```

Retain previous loaded rows when a refresh fails; do not replace them with an
incorrect empty state.

### Detail failure

```text
Could not open this clipping
Retry
```

The list remains usable. A `CLIPPING_NOT_FOUND` caused by external deletion
removes the stale row after confirmation from a refreshed list.

## 8. Source card

### FR-SOURCE-CARD-001

The source card is outside the editable document and contains:

- Canonical image when asset state is ready.
- Provenance line.
- `Open source` or source-unavailable text.
- Read-only click-to-expand action.
- Asset warning when missing.

### FR-SOURCE-CARD-002: Image display

- `object-fit: contain`.
- Width fits the detail pane.
- Default maximum visual height: 45% of the detail viewport, minimum 240 CSS px
  when space allows.
- The full natural resolution is not used as a CSS size.
- Only one canonical full image is mounted for the selected clipping.
- Image decoding is asynchronous.

### FR-SOURCE-CARD-003: Expanded viewer

V1 may provide a read-only overlay with `Fit` and `100%` modes using the same
canonical media URL. It may pan/scroll but cannot crop, annotate, replace, or
export. Closing returns focus to the source image button.

If implementation cost threatens Phase 4B gates, expanded viewing may be
removed without changing aggregate behavior; the ordinary source card is
required.

### FR-SOURCE-CARD-004: Missing asset

When `assetState = missing` or media load returns missing/corrupt:

```text
Saved image is unavailable
Your note is still safe. LinkVault could not find or validate the clipping
image.
```

The title and note remain editable. Repair behavior is in specification 06.

## 9. Title field

### FR-TITLE-001

The title is a normal accessible text input above the editor with label
`Clipping note title`.

### FR-TITLE-002

- Uses the same draft and revision as Markdown.
- Trims surrounding whitespace on persistence.
- Does not modify interior whitespace or punctuation.
- Shows a character count only near the 200-scalar limit.
- Empty-after-trim or over-limit state blocks autosave and displays inline
  validation; the last persisted title remains safe.

### FR-TITLE-003

Pressing Enter in the title moves focus to the editor rather than inserting a
newline. Escape restores the last persisted title only after explicit
confirmation when local title edits exist.

## 10. Approved Markdown subset

V1 supports:

- Paragraphs.
- Headings levels 1–4.
- Bold.
- Italic.
- Strikethrough.
- Unordered lists.
- Ordered lists.
- Blockquotes.
- Links.
- Soft and hard line breaks.

V1 does not support:

- Images or attachments inside Markdown.
- Raw HTML.
- MDX, JSX, expressions, imports, exports, or components.
- Tables.
- Code blocks or inline code.
- Task lists.
- Footnotes.
- Embedded media, iframe, audio, or video.
- Automatic OCR/AI content.

### FR-MARKDOWN-001

The backend treats Markdown as UTF-8 text and applies size/NUL validation. It
does not execute or evaluate content.

### FR-MARKDOWN-002

The editor adapter serializes only the approved subset. Unsupported paste/input
is converted to allowed plain text/formatting or rejected with a non-destructive
message.

### FR-MARKDOWN-003: Links

Only explicit `http`, `https`, and `mailto` links are interactive in rendered
or editor preview surfaces. `javascript`, `data`, `vbscript`, file URLs, and
unknown schemes render as inert text. External opening uses the existing safe
Tauri opener path and requires user activation.

### FR-MARKDOWN-004: Paste

Paste priority inside the editor:

1. Plain text and allowed rich text converted to approved Markdown.
2. Unsupported HTML stripped to text.
3. Clipboard image/file payload rejected with:

   ```text
   Images aren't supported inside clipping notes.
   ```

The source card is the clipping image and cannot be replaced by paste.

## 11. Editor toolbar

Required controls, in order where layout permits:

```text
Undo
Redo
Heading
Bold
Italic
Strikethrough
Bulleted list
Numbered list
Blockquote
Link
```

At narrow widths, formatting controls may use an overflow menu while Undo/Redo
remain directly accessible.

No image, file, table, code, task-list, AI, or source-MDX control is shown.
A Markdown source toggle is optional only if the selected editor can guarantee
round-trip safety and product owner approves it during Phase 4A; it is not a V1
requirement.

## 12. Internal editor adapter

Binding interface:

```ts
export type ClippingNoteEditorProps = {
  documentId: string;
  initialMarkdown: string;
  readOnly?: boolean;
  autoFocus?: boolean;
  onMarkdownChange: (markdown: string) => void;
  onBlur: () => void;
  onReady?: () => void;
};

export type ClippingNoteEditorHandle = {
  focus: () => void;
  getMarkdown: () => string;
};
```

### FR-EDITOR-001

The adapter root is marked:

```text
data-editor-root="true"
aria-label="Clipping note editor"
```

so reader/global shortcuts can exclude it.

### FR-EDITOR-002

`documentId` identifies the editor document. The parent flushes the prior draft
before changing it. The adapter remounts or explicitly resets editor state so
content, undo history, selection, and composition cannot leak between clipping
IDs.

### FR-EDITOR-003

The adapter reports Markdown changes after a complete editor transaction and
does not emit partial composition strings as independently committed documents.

### FR-EDITOR-004

The adapter does not invoke Tauri directly. The save controller owns
persistence.

### FR-EDITOR-005

The adapter works offline and includes no telemetry, cloud collaboration,
remote schema, remote asset upload, or runtime CDN dependency.

## 13. Phase 4A editor compatibility gate

The exact editor package is intentionally unresolved until the spike. Phase 4A
must evaluate at least two current viable candidates; recommended candidates to
inspect include a Markdown-first React editor and a Lexical/ProseMirror-based
alternative with a reliable Markdown serializer.

### Spike fixture document

The same fixture is loaded, edited, serialized, reloaded, and compared:

```markdown
# Research note

This is **bold**, *italic*, and ~~removed~~ text.

- First point
- 第二點
  - Nested item

1. One
2. Two

> Quoted observation

[Source](https://example.com/path?q=test)

A paragraph entered with Chinese IME: 世界日報剪報測試。
```

### Required evaluation matrix

| Area | Required evidence |
|---|---|
| React | React 19 production build; Strict Mode mount/unmount/remount |
| Composition | Synthetic browser coverage plus visible Windows dev-harness user smoke; native Tauri IME remains a Phase 4B integration gate |
| Markdown | Approved subset round-trips without editor-specific syntax or semantic loss |
| Unsupported syntax | Raw HTML/MDX/image/table/code paste is stripped/rejected predictably |
| Undo/redo | Works across formatting and does not break after parent autosave state updates |
| Document switch | No stale content, selection, history, or composition crosses IDs |
| Accessibility | Keyboard toolbar, labels, focus order, pressed/disabled states, and visible focus |
| Theme | Light/dark, high contrast, focus-visible, disabled/read-only |
| Offline | No network request needed to load or edit |
| Bundle | Record raw and gzip production bundle delta |
| License | Compatible license and third-party notice requirements documented |
| Maintenance | Current release cadence/issues reviewed at evaluation time |
| Security | No executable MDX/raw HTML and safe link handling |

### Phase 4A artifacts

Commit:

```text
docs/evaluations/newspaper-clipping-editor-<date>.md
```

The report includes:

- Exact candidate package versions.
- Reproduction commands.
- Test fixture results.
- Bundle deltas.
- Native IME evidence.
- Selected candidate and rejected reasons.
- Adapter/plugin configuration.
- Known limitations.

Then update D-024 to Approved with the exact dependency range. Phase 4B remains
blocked until this is merged.

### Bundle review threshold

No permanent editor-specific hard cap is set before the spike. However, a
selected editor adding more than 500 KiB gzip to the primary startup chunk or
causing the Clippings editor to load eagerly on non-Clippings routes requires
explicit product/architecture approval. Prefer lazy loading the editor chunk
when the Clippings detail is opened.

## 14. Draft and autosave model

Parent-owned state:

```ts
type ClippingDraftState = {
  clippingId: string;
  persistedTitle: string;
  persistedMarkdown: string;
  draftTitle: string;
  draftMarkdown: string;
  revision: number;
  status: "clean" | "dirty" | "saving" | "failed" | "conflict";
  errorCode?: string | null;
  inFlight?: {
    submittedTitle: string;
    submittedMarkdown: string;
    expectedRevision: number;
  } | null;
};
```

### FR-AUTOSAVE-001: Dirty detection

Dirty is based on exact normalized title/Markdown comparison with the last
acknowledged persisted values. Editor cursor/selection changes do not mark
dirty.

### FR-AUTOSAVE-002: Debounce

Schedule save 800 ms after the latest valid title or Markdown change. Reset the
timer on each change. Invalid title or over-limit Markdown stays dirty with
inline validation and does not submit.

### FR-AUTOSAVE-003: One in-flight update

Only one update per clipping is in flight. If the user edits during saving:

- Keep current draft.
- Let the submitted snapshot resolve.
- On success, update persisted snapshot/revision to the submitted values.
- If current draft differs, remain dirty and schedule/perform the next save.
- Never replace current draft with an older response payload.

### FR-AUTOSAVE-004: Success

Show `Saved` only when current draft equals the latest acknowledged persisted
values. Update list title/excerpt/updated time in place and emit/consume the
invalidation event without disrupting focus.

### FR-AUTOSAVE-005: Failure

- Preserve draft, editor selection where possible, and persisted revision.
- Show `Save failed` with `Retry`.
- Do not retry in an unbounded automatic loop.
- One controlled retry may occur for classified transient database busy/service
  failure after the existing application delay policy; user-visible state must
  remain truthful.

### FR-AUTOSAVE-006: Flush boundaries

Attempt immediate save when dirty before:

- Selecting another clipping.
- Search/sort removes current clipping.
- Returning to list on narrow layout.
- Opening source.
- Navigating to another app view.
- Detail/editor unmount.
- Window/application blur.
- Cooperative native close request.
- Deleting the clipping.

### FR-AUTOSAVE-007: Navigation guard

If a required flush fails, block the pending navigation and show:

```text
Your changes aren't saved
Retry saving
Stay here
Discard changes and continue
```

`Discard changes and continue` is explicit, destructive, and restores the last
persisted values before completing the pending action. No background list/search
response may choose it automatically.

### FR-AUTOSAVE-008: Close limitation

A normal Tauri close request may wait briefly for one update. Forced process
termination, OS kill, or power loss inside the 800 ms debounce window remains a
known V1 limitation and must be documented honestly. V1 does not add a second
local draft journal.

## 15. Revision conflict handling

### FR-CONFLICT-001

On `CLIPPING_REVISION_CONFLICT`:

1. Preserve local draft in state.
2. Fetch latest clipping detail.
3. Stop ordinary autosave.
4. Show `Changed elsewhere` with the latest saved updated time.
5. Offer:

   ```text
   Keep my changes
   Use saved version
   Copy my draft
   ```

### FR-CONFLICT-002: Keep my changes

After explicit activation, submit current local title/Markdown using the latest
revision. This is an intentional overwrite of the latest saved title/note. If it
conflicts again, remain in conflict state and repeat; never loop automatically.

### FR-CONFLICT-003: Use saved version

After confirmation, replace local draft/editor document with the latest fetched
values and revision, clear undo history for the old document state where the
adapter permits, and return to clean.

### FR-CONFLICT-004: Copy my draft

Copies title and Markdown in a readable plain-text/Markdown format without
changing conflict state. Clipboard failure is surfaced safely.

### AC-EDITOR-001

Given two views loaded at revision 5

When both edit and autosave

Then one reaches revision 6

And the other enters conflict with its local draft intact

And no automatic response overwrites either complete document.

## 16. Detail switching and stale responses

### FR-DETAIL-001

Every detail request has a generation/token tied to selected ID. A response for
an older selection is ignored and cannot replace the current editor.

### FR-DETAIL-002

Do not clear the current readable detail immediately while a same-ID refresh is
in flight. Show a subtle refresh state and retain content unless the row is
confirmed deleted.

### FR-DETAIL-003

Changing clipping ID requires successful flush or explicit discard. Only then
may the old editor unmount and new detail load.

### FR-DETAIL-004

The deep `Open note` path sets `autoFocus=true` after detail and editor are
ready. Ordinary list refresh/reselection never steals editor focus.

## 17. Editor performance and lazy loading

### FR-PERF-EDITOR-001

The selected editor bundle is lazy-loaded only when a clipping detail requires
editing. Download editions, Newspaper library, and other provider routes must
not eagerly initialize editor code.

### FR-PERF-EDITOR-002

Typing and IME composition do not synchronously invoke Tauri, serialize the
entire clippings list, regenerate thumbnails, or rerender all virtual rows.

### FR-PERF-EDITOR-003

A 2 MiB boundary fixture must remain editable enough to show validation and
save behavior, but V1 performance targets are optimized for ordinary notes.
The UI does not load full Markdown for 500 list rows.

### FR-PERF-EDITOR-004

Only selected clipping detail and one full canonical image are mounted. Changing
list search does not remount the editor when the selected clipping remains in
results.

## 18. Accessibility

### FR-A11Y-LIBRARY-001

The list uses semantic selectable rows/listbox or an equally accessible pattern
with clear selected state. It must not place one invisible full-row button over
nested interactive controls in a way that breaks keyboard access.

### FR-A11Y-LIBRARY-002

Search, sort, list, source card, title, editor toolbar, editor body, save state,
conflict actions, and delete overflow follow a predictable focus order.

### FR-A11Y-LIBRARY-003

Save-state changes use a polite live region only for failure/conflict and final
saved transition after a meaningful pause; normal typing does not announce
`dirty` on every character.

### FR-A11Y-LIBRARY-004

Formatting controls expose pressed/current states and tooltips, operate by
keyboard, and do not steal selection unexpectedly.

### FR-A11Y-LIBRARY-005

Chinese IME composition and editor key handling take priority over global
reader/app shortcuts while the editor owns focus.

## 19. Browser and native test matrix

### List tests

- 0, 1, 8, 50, 51, and 500 rows.
- Sparse placeholders and page fetch alignment.
- Stale search responses.
- Search escaping and Chinese text.
- All sort options with deterministic ties.
- Scroll anchor across invalidation.
- Visible-only thumbnail ensure.
- Thumbnail busy/unavailable/failure.
- Deep target and missing target.
- Delete selection fallback.
- Narrow list/detail transition.

### Editor automated tests

- Approved Markdown fixture round-trip.
- Empty note.
- Title-only change.
- Note-only change.
- Simultaneous title/note change.
- 800 ms debounce with fake timers.
- Edit during in-flight save.
- Failure and manual retry.
- Navigation flush success/failure/discard.
- Revision conflict and all three actions.
- Stale detail/update responses.
- Document switch isolation.
- Unsupported paste.
- Safe/unsafe links.
- Strict Mode mount/unmount.
- Lazy chunk not loaded on other routes.

### Native Windows tests

- Chinese IME composition, candidate selection, punctuation, Enter, backspace,
  undo, redo, document switch, blur, and autosave.
- Light/dark theme and 100/125/150/200% scaling.
- Keyboard-only toolbar and editor use.
- Labels, focus order, and visible focus in keyboard-only operation.
- Normal close with dirty note and failed-save navigation guard.

## 20. Phase 4A exit gate

Phase 4A is complete only when:

- Phase 1 is merged.
- At least two current candidates are evaluated using the same fixture and
  criteria.
- The evaluation report is committed.
- D-024 is updated to Approved with exact dependency/version/configuration.
- Rejected candidate dependencies and experimental code are removed.
- Selected adapter skeleton/tests compile under React 19 and Strict Mode.
- Synthetic composition and visible Windows dev-harness user acceptance are
  recorded; native Tauri IME remains a Phase 4B exit gate.
- Bundle/license/security evidence is recorded.
- The coding agent stops before production Clippings view integration.

## 21. Phase 4B exit gate

Phase 4B is complete only when:

- Phases 3 and 4A are merged.
- The Clippings sidebar item and view are production-enabled.
- Reader `Open note` action navigates to the created clipping.
- Paged/virtualized list, visible-only thumbnails, detail source card, title,
  selected editor, search/sort, autosave, navigation guard, and conflict UI are
  complete.
- All automated list/editor tests pass at 8, 50, and 500 clipping sizes.
- Native Chinese IME and DPI tests pass.
- Editor code is lazy-loaded and does not regress unrelated routes.
- Existing UI, visual, Newspaper performance/browser performance, architecture,
  persistence, Rust, frontend build, and release gates remain green.
- Deletion and exact source-return implementation remain for Phase 5; Phase 4B
  may render disabled/appropriate source state but must not fake completion.
- The coding agent stops.
