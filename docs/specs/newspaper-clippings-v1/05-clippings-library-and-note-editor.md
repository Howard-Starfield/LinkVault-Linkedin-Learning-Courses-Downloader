# Newspaper Clippings V1: library and Markdown note editor

**Status:** Approved

**Primary implementation phases:** Phase 4A editor evaluation, Phase 4B
integration, and Phase 4C durability hardening
production integration

**Related decisions:** D-001, D-010 through D-015, D-018 through D-026, D-028,
D-032 through D-034,
D-029

## 1. Purpose

This specification defines the dedicated Clippings view, paged and virtualized
gallery, detail clipping header, search and sorting, frontend data contracts, internal
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

### 3.6 Ranked search queries

Search uses dedicated commands rather than overloading the ordinary list query.
The backend owns ranking, candidate bounds, and confident-result exclusion.

```ts
export type NewspaperClippingMatchField =
  | "title"
  | "note"
  | "edition"
  | "date"
  | "page";

export type NewspaperClippingSearchSnippet = {
  field: NewspaperClippingMatchField;
  parts: Array<{
    text: string;
    highlighted: boolean;
  }>;
};

export type NewspaperClippingSearchResult = {
  clipping: NewspaperClippingSummary;
  matchedFields: NewspaperClippingMatchField[];
  snippets: NewspaperClippingSearchSnippet[];
  possibleMatch: boolean;
};

export type SearchNewspaperClippingsRequest = {
  query: string;
  offset: number;
  limit: 50;
};

export type SearchNewspaperClippingsPage = {
  items: NewspaperClippingSearchResult[];
  total: number;
  offset: number;
  limit: 50;
  noteSearchApplied: boolean;
  revision: number;
};

export type SearchPossibleNewspaperClippingsRequest = {
  query: string;
};

export type SearchPossibleNewspaperClippingsResponse = {
  items: NewspaperClippingSearchResult[];
  limit: 25;
  revision: number;
};
```

Commands:

```text
search_newspaper_clippings
search_possible_newspaper_clippings
```

`parts` avoids ambiguous byte/Unicode offsets and is rendered as plain text;
React must not interpret snippet text as HTML. Confident results always have
`possibleMatch: false`; the second command always returns `true`, never returns
more than 25 items, and internally excludes every confident match for the same
normalized query. Neither response exposes a numeric relevance or similarity
score. `noteSearchApplied` is authoritative after backend normalization and
drives the short-query helper; React does not infer that notes were searched.

### 3.7 Snapshot-location Settings queries

```ts
export type NewspaperSnapshotRootStatus =
  | "unchecked"
  | "connected"
  | "offline"
  | "marker_mismatch";

export type NewspaperSnapshotRootSummary = {
  rootId: string;
  kind: "download_snapshot" | "legacy_managed";
  displayPath: string;
  status: NewspaperSnapshotRootStatus;
  lastCheckedAt?: number | null;
};

export type ReconnectNewspaperSnapshotRootResult =
  | { status: "cancelled" }
  | { status: "connected"; root: NewspaperSnapshotRootSummary };
```

Commands:

```text
list_newspaper_snapshot_roots
check_newspaper_snapshot_root
reconnect_newspaper_snapshot_root
open_newspaper_snapshot_root
```

All action inputs contain only `rootId`. `displayPath` is presentation data and
must never be accepted back as filesystem authority. Reconnect owns the native
directory picker at the Tauri boundary and returns `cancelled` without mutation
when the user dismisses it. Legacy managed roots are listed for diagnostics but
cannot be reconnected to a download destination.

## 4. Gallery and full-page detail

### Desktop gallery

The Clippings route opens as a responsive, row-virtualized image gallery. At
the default desktop content width it displays four thumbnails per row. The
column count responds from one through six as available width changes while
preserving each clipping's bounded source aspect ratio. Only visible or
near-visible thumbnails are generated and mounted.

Selecting a thumbnail opens a separate full-page note document. The app-level
Clippings toolbar becomes a compact row containing `Back` and the editable note
title; the gallery-only search box is absent. The detail uses one centered
writing column containing, in order:

1. fixed read-only clipping image and provenance;
2. continuous Tiptap Markdown body;
3. quiet bottom footer with save state and history controls.

The detail does not render a split pane, a duplicate internal header, or an
editor card. Dirty-navigation guards are identical at every width.

When the Clippings toolbar query is non-empty, a search-results surface takes
over the provider-owned main content area at every width. The toolbar remains
visible. The ordinary gallery/detail composition and its Tiptap instance retain
their state but are removed from focus/accessibility navigation; search does
not destroy a draft merely because the first character was typed.

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Clippings   [ Search clipping notes…                              ] │
├──────────────────────────────────────────────────────────────────────┤
│ 18 results · Relevance                                               │
│ [Title] [Note]  Result title                                         │
│ matching plain-text note excerpt with safe visual emphasis           │
│ Edition · date · page                                                 │
│                                                                      │
│ Possible matches                                                     │
│ [Possible match] [Edition] …                                         │
└──────────────────────────────────────────────────────────────────────┘
```

### FR-LAYOUT-001

The gallery owns its virtual scroll surface. The separate detail owns its own
vertical scroll; scrolling a long note cannot mutate the retained gallery
position.

### FR-LAYOUT-002

Returning from detail or source reader restores list query, sort, selected ID,
and scroll position when still valid.

## 5. Gallery loading and virtualization

The list follows the existing Newspaper library’s bounded loading pattern.

```text
PAGE_SIZE = 50
ESTIMATED_ROW_HEIGHT = responsive to column width and source aspect ratio
OVERSCAN = 2 rows
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
- Title, Edition, Date, and Page matching starts at one Unicode scalar value.
  Note matching starts at three. With a one- or two-scalar query, show
  `Type 3 characters to search notes` and do not produce a Note tag or snippet.
- After the 200 ms debounce, a non-empty query replaces the ordinary
  list/detail presentation with full-width ranked results without unmounting
  or clearing the active editor draft.
- Each request has a monotonically increasing frontend generation. Stale
  responses are discarded and duplicate IDs are removed across pages.
- Confident results load 50 at a time as the virtual scroll approaches its
  tail. At most the current and one near-future page are in flight.
- Only after the confident result count is exhausted may the frontend request
  one fuzzy page. It is headed `Possible matches`, contains at most 25 unique
  rows, and is never interleaved into confident ranking.
- Clicking a result flushes/resolves any dirty note before changing detail. A
  failed flush keeps the current note and query visible with a retry action.
- Returning from a result restores the exact query, loaded-page boundary,
  scroll anchor, and focused result when it still exists.

### Match fields and snippets

- Result tags are factual and cumulative: `Title`, `Note`, `Edition`, `Date`,
  and `Page`.
- Fuzzy rows also display `Possible match`; no numeric confidence is shown.
- Confident matching covers all five fields. Fuzzy matching covers only Title,
  Note, and Edition. Date and Page are literal and never approximated.
- A safe bounded plain-text snippet is selected around a Note match. Markdown
  syntax, raw HTML/MDX, and executable content are never rendered from a search
  snippet.
- Highlighting uses the returned plain-text `parts`; the UI does not reconstruct
  byte or Unicode offsets and never uses `dangerouslySetInnerHTML`.
- A title or provenance-only result does not invent a Note tag or note match.

### Relevance contract

Confident ordering is deterministic:

1. Exact normalized title.
2. Normalized title prefix.
3. Weighted FTS relevance, with Title weighted above Note and Note above
   Edition.
4. Literal Date/Page match contribution below text-field matches.
5. `updated_at DESC`, then clipping ID as final ties.

FTS weights are frozen only after the committed mixed English/Chinese golden
ranking fixture proves representative ordering. The UI does not expose the
internal numeric score.

Possible-match candidate generation uses the trigram index and a bounded
candidate/window limit. Similarity evaluation never reads every full note per
keystroke. It requires at least four Unicode scalar values and uses a documented
Unicode-normalized edit-distance threshold. Confident IDs are excluded before
the maximum 25 fuzzy rows are returned.

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

While search is non-empty, the sort control reads `Relevance` and ordinary sort
choices are disabled/hidden. Clearing search restores the previous list sort.

### AC-LIBRARY-001

Given `%`, `_`, backslash/escape, apostrophe, Chinese text, and mixed date/page
queries

When search runs

Then values are treated literally, results are paged deterministically, and no
SQL syntax is interpolated.

### AC-LIBRARY-002

Given confident matches, typo candidates, one- to four-character queries,
mixed English/Chinese text, a two-megabyte note, and more than 50 results

When the user types, scrolls through confident pages, reaches Possible matches,
opens a result, and returns

Then ranking/tags/snippets are factual, one- and two-character queries exclude
Note and explain its three-character minimum, no more than 25 fuzzy rows appear,
stale pages cannot append, full notes are not linearly scanned for fuzzy
scoring, and query/scroll/draft state is preserved.

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

## 8. Read-only clipping header

### FR-SOURCE-CARD-001

The clipping header is outside the editable ProseMirror document but shares
the same full-width writing column. It contains:

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
removed without changing aggregate behavior; the ordinary clipping header is
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

The title is a normal accessible text input beside `Back` in the app-level top
bar with label `Clipping note title`. It is not duplicated in the writing
column.

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

The clipping header is the clipping image and cannot be replaced by paste.

## 11. Editor commands and contextual toolbar

The full-page editor does not use a permanent boxed formatting strip. Required
controls are divided by intent:

- A quiet bottom-right footer exposes save state, Undo, and Redo without
  interrupting the editor body.
- Typing `/` at a text-block start or after whitespace opens a body-portaled
  command menu above the application shell for
  paragraph, Heading 1-4, task list, bulleted list, numbered list, blockquote,
  and horizontal rule.
- Selecting non-empty text exposes Bold, Italic, Strikethrough, and Link in a
  toolbar above the selection.
- The selection toolbar remains hidden during pointer drag and appears only
  after pointer release. It anchors to the document-order beginning of the
  selection, including reverse selections, then flips/shifts within the
  viewport when required.

Slash commands support alias-aware fuzzy ranking, pointer selection, Arrow
Up/Down, Enter, and Escape. The best result is selected as the query changes,
but typing never executes it automatically. Composition suppresses both
transient menus.

No image, file, table, code, AI, or source-MDX control is shown.
A Markdown source toggle is optional only if the selected editor can guarantee
round-trip safety and product owner approves it during Phase 4A; it is not a V1
requirement.

### 11.1 Visual reference and license boundary

The compact icon/title/description interaction is informed by Novel's
Apache-2.0 slash-command design, while colors and spacing are independently
implemented with LinkVault-owned tokens. NoteGen is a visual/behavioral
reference only because its implementation is GPL-3.0; no NoteGen CSS or source
is copied. Paid Tiptap templates are not used.

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

Tiptap is a rendering/editing adapter, not a persistence authority. `onUpdate`
serializes the approved document subset with `getMarkdown()`. Parent autosave
sends that Markdown to Rust, which validates and commits it to SQLite. LinkVault
does not persist Tiptap-specific JSON or HTML. The local `note.md` file is a
post-commit export projection governed by D-035, never editor state or an input
to this adapter.

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
  status:
    | "loading-recovery"
    | "clean"
    | "dirty"
    | "saving"
    | "failed"
    | "conflict"
    | "recovered";
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

Continuous typing must not postpone canonical durability indefinitely. A
separate maximum-wait timer submits the latest valid canonical snapshot no more
than 5 seconds after the first unsaved valid change, without creating a second
in-flight update.

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

`Saved` means the SQLite canonical commit and checkpoint acknowledgement
succeeded. A subsequent `note.md` projection failure is recorded for repair and
must not provoke an invalid optimistic-revision retry of an already committed
save.

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

### FR-AUTOSAVE-008: Native recovery checkpoint

D-034 supersedes the earlier no-journal limitation. Independently of canonical
autosave, coalesce a recovery checkpoint after 500 ms of quiet time and at
least every 2 seconds during continuous typing. The checkpoint carries
clipping ID, canonical base revision, an unguessable mounted-writer session,
monotonic sequence, title, and Markdown. Only one checkpoint write is in flight;
newer visible edits become queued-latest work.

Recovery checkpoints accept an empty or otherwise canonically invalid title
when within the approved 4 KiB title / 4 MiB Markdown recovery envelope. They do
not update the clipping revision, canonical metadata, list excerpt, or search
index.

### FR-AUTOSAVE-009: Cooperative close and exit

Window X prevents native close and requests the latest durability state. On a
successful canonical flush or acknowledged matching recovery checkpoint, hide
the existing main WebView. Tray **Quit**, application exit, and updater exit use
the same handshake and terminate only after success. A canonical revision
conflict may proceed only when the exact newest visible draft has a matching
durable checkpoint, with recovery copy shown before exit and both versions
offered next launch. Failure, an uncheckpointed/stale conflict, missing/stale
acknowledgement, or timeout keeps the application alive and shows/focuses the
main window. Timeout never means discard.

The database writer remains available until the renderer acknowledges the
exact native request token. Only the subsequently confirmed exit may stop crop
work, drain accepted work, and shut down the writer.

### FR-AUTOSAVE-010: Recovery classification

Before enabling edits, load any checkpoint for the clipping and compare its
base revision and bytes with the canonical note:

- equal bytes: clear the redundant matching checkpoint;
- matching base revision with different bytes: offer the recovered draft;
- advanced canonical revision or different unresolved writer session: enter a
  recovery conflict and preserve both complete documents;
- malformed or over-envelope row: fail safely without displaying raw database
  or path details and preserve the row for forward repair.

No recovery draft is silently applied, overwritten, indexed, or discarded.

### FR-AUTOSAVE-011: Sequence-safe acknowledgement

A canonical save acknowledges the writer session and submitted sequence. The
backend atomically saves canonical title/Markdown/revision/FTS and clears only a
matching checkpoint no newer than that submission. A stale async completion
cannot clear a newer visible draft.

### FR-AUTOSAVE-012: Unmount is not durability authority

React cleanup may unregister the surface and stop timers, but it cannot claim a
final save succeeded. Application-controlled navigation flushes before unmount;
native code owns close/exit prevention. Browser storage, `beforeunload`, blur,
and fire-and-forget cleanup are never the source of truth.

### FR-AUTOSAVE-013: Second-launch activation

Installed LinkVault permits one desktop process. A second launch activates the
existing main window. If a clipping detail is open, the frontend first invokes
the registered durability flush, then reloads canonical detail and gallery
pages. A failed flush or unresolved conflict preserves the current editor draft
and blocks that refresh.

## 15. Revision conflict handling

### FR-CONFLICT-001

On `CLIPPING_REVISION_CONFLICT`:

1. Preserve local draft in state.
2. Fetch latest clipping detail.
3. Stop ordinary autosave.
4. Continue accepting visible edits and checkpoint them under the current
   writer session; conflict never freezes or discards later typing.
5. Show `Changed elsewhere` with the latest saved updated time.
6. Offer:

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

Search, sort, gallery, clipping header, title, editor controls, editor body, save state,
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
- 5-second maximum canonical wait under continuous typing.
- 500 ms quiet / 2-second maximum recovery checkpoint coalescing.
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
- Window X hide, tray Quit exit, timeout/failure blocking, and isolated-profile
  crash/restart recovery.

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
- Paged/virtualized gallery, visible-only thumbnails, detail clipping header, title,
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

## 22. Phase 4C exit gate

Phase 4C is complete only when:

- Phase 4B behavior is available on the reviewed Phase 4C base.
- D-034 schema-v6 migration and recovery contracts pass fresh, populated-v5,
  current-v6, failure, and future-version fixtures.
- Canonical autosave, checkpoint coalescing, conflict capture, recovery
  classification, and sequence-safe acknowledgement pass deterministic tests.
- Window X safely hides; tray Quit/application/updater exit safely terminate;
  failed, uncheckpointed/stale-conflict, stale-token, missing-owner, and
  timed-out attempts remain open. A canonical conflict proceeds only with the
  exact newest recovery checkpoint durable.
- Database shutdown occurs only after an exact confirmed durability token.
- Search, list metadata, and FTS never expose a recovery draft.
- Every ownership and hard line-size budget in the Phase 4C work order passes its
  structural gate.
- Release measurements cover write counts, latency, memory, bundle size, and
  SQLite/WAL growth without unacceptable regression.
- Installed Windows lifecycle and isolated-profile crash-recovery UAT is
  recorded separately from browser automation.
- All existing Phase 4B, persistence, architecture, UI, browser, Rust, and
  release gates remain green, generated output is cleaned, and the coding agent
  stops before Phase 5.
