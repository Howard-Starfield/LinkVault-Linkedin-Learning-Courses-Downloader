# Newspaper Clippings V1: domain, persistence, and managed assets

**Status:** Approved

**Primary implementation phases:** Phase 1 foundation and Phase 4C durability
migration

**Related decisions:** D-004 through D-012, D-014 through D-022, D-027, D-028,
D-032 through D-034

## 1. Purpose

This specification defines the clipping aggregate, SQLite schema, migration
contract, repository operations, managed asset layout, creation/deletion state
machines, startup recovery, reset behavior, validation limits, and persistence
acceptance criteria.

Phase 1 must not add reader selection UI, WYSIWYG dependencies, OCR, or AI. It
establishes a safe persistence and filesystem foundation that later phases can
consume.

## 2. Ownership and module boundary

New code belongs beneath the Newspaper provider and shared application services
only where the concern is already application-owned.

Target ownership:

```text
apps/desktop/src-tauri/src/
├─ app/
│  └─ storage.rs                         add clipping-root resolver only
└─ providers/newspaper/
   ├─ clipping_models.rs                 persisted/domain DTOs
   ├─ clipping_repository.rs             SQLite reads and writer closures
   ├─ clipping_assets.rs                 managed paths and file lifecycle
   ├─ clipping_recovery.rs               startup reconciliation and cleanup
   ├─ clipping_service.rs                orchestration boundary
   ├─ media_protocol.rs                  clipping media variants
   ├─ commands.rs                        thin IPC adapters
   ├─ storage.rs                         provider schema installation
   └─ mod.rs                             provider-owned module declarations
```

Exact filenames may change during review, but responsibility may not drift into
`lib.rs`, React, another provider, or a new generic notes subsystem.

### FR-DOMAIN-001

The Newspaper provider owns clipping domain data and operations. Shared app
storage may resolve the managed root; shared database lifecycle and
`DatabaseWriter` retain their existing ownership.

### FR-DOMAIN-002

Tauri commands validate serialization-level input and delegate. They do not
contain SQL, image processing, or filesystem state machines.

## 3. Aggregate model

A V1 clipping is one aggregate with this conceptual shape:

```rust
pub struct NewspaperClipping {
    pub id: String,

    pub source_job_id: Option<String>,
    pub source_page_id: Option<String>,
    pub source_media_version_snapshot: i64,
    pub source_kind_snapshot: ClippingSourceKind,
    pub source_mime_type_snapshot: String,
    pub source_checksum_snapshot: Option<String>,

    pub edition_code_snapshot: String,
    pub edition_name_snapshot: String,
    pub publication_date_snapshot: String,
    pub page_number_snapshot: String,

    pub source_pixel_width: u32,
    pub source_pixel_height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub crop_width: u32,
    pub crop_height: u32,

    pub asset_root_id: String,
    pub asset_relative_path: String,
    pub asset_mime_type: String,
    pub asset_pixel_width: u32,
    pub asset_pixel_height: u32,
    pub asset_byte_count: u64,
    pub asset_checksum_sha256: String,
    pub asset_version: u32,
    pub asset_state: ClippingAssetState,
    pub asset_error_code: Option<String>,

    pub title: String,
    pub note_markdown: String,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}
```

Enums:

```rust
pub enum ClippingSourceKind {
    Original,
    Optimized,
}

pub enum ClippingAssetState {
    Creating,
    Ready,
    Missing,
    DeletePending,
}
```

### Aggregate invariants

- `id` is also the idempotency key for one accepted save attempt.
- `source_job_id` and `source_page_id` may become null after source deletion.
- Provenance snapshots never become null after creation.
- Crop dimensions equal canonical asset dimensions in V1.
- `asset_state = ready` means the canonical file exists, is a regular
  non-symlink file inside the managed root, has supported MIME, decodes, matches
  dimensions, and has the stored SHA-256.
- `asset_state = missing` preserves the row and note while recording a safe
  integrity code.
- `revision` starts at 1 and increments on every successful mutable title/note
  update. Source provenance and crop geometry are immutable in V1.
- A clipping row is visible to ordinary list queries only in `ready` or
  `missing` state. `creating` and `delete_pending` are recovery states.

## 4. SQLite schema

The implementation must add a provider-owned table equivalent to the following.
Column names and constraints are binding unless an approved migration review
records a necessary SQLite compatibility adjustment.

```sql
CREATE TABLE IF NOT EXISTS newspaper_clipping_roots (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('legacy_managed', 'download_snapshot')),
    locator TEXT NOT NULL,
    locator_key TEXT NOT NULL COLLATE NOCASE UNIQUE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS newspaper_clippings (
    id TEXT PRIMARY KEY NOT NULL,

    source_job_id TEXT,
    source_page_id TEXT,
    source_media_version_snapshot INTEGER NOT NULL
        CHECK (source_media_version_snapshot > 0),
    source_kind_snapshot TEXT NOT NULL
        CHECK (source_kind_snapshot IN ('original', 'optimized')),
    source_mime_type_snapshot TEXT NOT NULL,
    source_checksum_snapshot TEXT,

    edition_code_snapshot TEXT NOT NULL,
    edition_name_snapshot TEXT NOT NULL,
    publication_date_snapshot TEXT NOT NULL,
    page_number_snapshot TEXT NOT NULL,

    source_pixel_width INTEGER NOT NULL
        CHECK (source_pixel_width > 0),
    source_pixel_height INTEGER NOT NULL
        CHECK (source_pixel_height > 0),
    crop_x INTEGER NOT NULL
        CHECK (crop_x >= 0),
    crop_y INTEGER NOT NULL
        CHECK (crop_y >= 0),
    crop_width INTEGER NOT NULL
        CHECK (crop_width > 0),
    crop_height INTEGER NOT NULL
        CHECK (crop_height > 0),

    asset_root_id TEXT NOT NULL,
    asset_relative_path TEXT NOT NULL,
    asset_mime_type TEXT NOT NULL
        CHECK (asset_mime_type = 'image/webp'),
    asset_pixel_width INTEGER NOT NULL
        CHECK (asset_pixel_width > 0),
    asset_pixel_height INTEGER NOT NULL
        CHECK (asset_pixel_height > 0),
    asset_byte_count INTEGER NOT NULL
        CHECK (asset_byte_count > 0),
    asset_checksum_sha256 TEXT NOT NULL,
    asset_version INTEGER NOT NULL DEFAULT 1
        CHECK (asset_version > 0),
    asset_state TEXT NOT NULL
        CHECK (asset_state IN ('creating', 'ready', 'missing', 'delete_pending')),
    asset_error_code TEXT,

    title TEXT NOT NULL,
    note_markdown TEXT NOT NULL DEFAULT '',
    revision INTEGER NOT NULL DEFAULT 1
        CHECK (revision > 0),

    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,

    FOREIGN KEY (source_job_id)
        REFERENCES newspaper_jobs(id)
        ON DELETE SET NULL,
    FOREIGN KEY (source_page_id)
        REFERENCES newspaper_pages(id)
        ON DELETE SET NULL,
    FOREIGN KEY (asset_root_id)
        REFERENCES newspaper_clipping_roots(id)
        ON DELETE RESTRICT,

    CHECK (crop_x + crop_width <= source_pixel_width),
    CHECK (crop_y + crop_height <= source_pixel_height),
    CHECK (asset_pixel_width = crop_width),
    CHECK (asset_pixel_height = crop_height),
    CHECK (
        (asset_state = 'missing' AND asset_error_code IS NOT NULL)
        OR
        (asset_state != 'missing')
    )
);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_updated
    ON newspaper_clippings(updated_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_created
    ON newspaper_clippings(created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_publication
    ON newspaper_clippings(
        publication_date_snapshot DESC,
        edition_code_snapshot,
        page_number_snapshot,
        id
    );

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_title
    ON newspaper_clippings(title COLLATE NOCASE, id);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_source_page
    ON newspaper_clippings(source_page_id);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_asset_state
    ON newspaper_clippings(asset_state, updated_at);

CREATE INDEX IF NOT EXISTS idx_newspaper_clippings_asset_root
    ON newspaper_clippings(asset_root_id, asset_state, updated_at);

CREATE VIRTUAL TABLE IF NOT EXISTS newspaper_clippings_fts USING fts5(
    title,
    note_markdown,
    edition_name_snapshot,
    edition_code_snapshot,
    content='newspaper_clippings',
    content_rowid='rowid',
    tokenize='trigram'
);
```

### Schema rationale

- Source foreign keys support exact navigation while available.
- Denormalized fields preserve provenance after `SET NULL`.
- The canonical asset checksum detects silent replacement or corruption.
- `asset_state` makes cross-filesystem/database operations recoverable.
- No editor JSON, OCR text, AI output, tags, arbitrary attachments, or remote
  identifiers are added.
- `newspaper_clippings_fts` is a derived search accelerator, never title/note
  source of truth. Insert/update/delete triggers keep its external-content rows
  synchronized in the same SQLite transaction as the clipping row.
- Search joins back to `newspaper_clippings` and exposes only `ready` or
  `missing` rows. Index corruption or absence cannot delete or rewrite a note.

### Recovery-checkpoint table (schema v6)

D-034 adds one local, recovery-only row per clipping:

```sql
CREATE TABLE newspaper_clipping_note_drafts (
    clipping_id TEXT PRIMARY KEY
        REFERENCES newspaper_clippings(id) ON DELETE CASCADE,
    base_revision INTEGER NOT NULL CHECK (base_revision >= 1),
    writer_session_id TEXT NOT NULL,
    writer_sequence INTEGER NOT NULL CHECK (writer_sequence >= 1),
    draft_title TEXT NOT NULL,
    draft_markdown TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
```

This table is not canonical note storage and has no FTS trigger. A same-session
checkpoint replaces only a lower sequence. A different session must first load
and classify the existing recovery row and may not overwrite it silently.
Backend validation allows at most 4 KiB of UTF-8 title bytes and 4 MiB of UTF-8
Markdown bytes. Draft bytes never appear in path fields, logs, diagnostics,
search results, list excerpts, or canonical `updated_at`.

## 5. Schema-version and migration contract

Phase 1 introduced the clipping table at schema version 3. D-032 advances the
application to schema version 4, adds the root registry and `asset_root_id`, and
rebuilds populated v3 clipping tables after the existing verified backup.
D-019 then advances the application to schema version 5, creates the external-
content FTS table and synchronization triggers, and issues an FTS `rebuild`
only after the canonical clipping rows are present. D-034 advances the
application to schema version 6 and adds the recovery-checkpoint table after the
same verified backup boundary. The verified backup occurs before any pending
migration when upgrading an older database.

### FR-MIGRATION-001

A fresh database initializes directly with `newspaper_clippings` and all
indexes at the new current schema version.

### FR-MIGRATION-002

A populated database below the new version receives the existing verified
online backup before provider schema installation creates the clipping table.

### FR-MIGRATION-003

The supported `PRAGMA user_version` is written only after the Newspaper schema
and every existing provider migration succeed.

### FR-MIGRATION-004

Opening an already-current database through the runtime API performs no schema
creation or mutation.

### FR-MIGRATION-005

A database newer than the supported version remains rejected before backup or
schema mutation.

### FR-MIGRATION-006

Schema-v5 installation verifies `ENABLE_FTS5`, creates all external-content
triggers, rebuilds the index from existing rows, and proves row-count/content
parity before writing `PRAGMA user_version = 5`. A failed create, trigger, or
rebuild rolls back and preserves the verified backup. Repair may drop and
rebuild only the derived FTS objects while leaving clipping rows untouched.

### FR-MIGRATION-007

Schema-v6 installation creates and verifies the exact recovery table,
constraints, and `ON DELETE CASCADE` foreign key before writing
`PRAGMA user_version = 6`. It does not add an FTS trigger, mutate canonical
clipping rows, or create recovery rows during migration. Failure rolls back
with `user_version` below 6 and preserves the verified backup.

### Migration evidence

Tests must prove:

- Legacy representative LinkedIn, Coursera, Newspaper job/page, reading
  progress, and settings rows survive migration.
- The backup passes integrity check and contains representative pre-migration
  data.
- The clipping table has both `ON DELETE SET NULL` source foreign keys and the
  `ON DELETE RESTRICT` root foreign key.
- Populated v3 clipping rows retain notes/assets and are backfilled to the
  stable `legacy-managed-v1` root.
- `PRAGMA foreign_key_check` is empty after migration.
- Re-running initialization is idempotent.
- A failed migration leaves `user_version` below the target and preserves the
  verified backup.
- Fresh, populated-v5, already-v6, failed-v6, and future-version fixtures prove
  the recovery table contract and preserve representative provider data.

## 6. Validation and storage limits

Backend validation is authoritative.

| Field | Limit/rule |
|---|---|
| Clipping/operation ID | Canonical lowercase UUID string, maximum 36 characters; no path separators |
| Title | Trimmed; 1–200 Unicode scalar values; maximum 800 UTF-8 bytes |
| Note Markdown | 0–2,097,152 UTF-8 bytes; no NUL |
| Edition code | 1–32 UTF-8 bytes |
| Edition name | 1–256 UTF-8 bytes |
| Publication date | Exact `YYYY-MM-DD` snapshot from source job |
| Page number | 1–64 UTF-8 bytes |
| Source MIME | One of supported registered page image MIME types |
| SHA-256 | Exactly 64 lowercase hexadecimal characters |
| Asset version | Starts at 1; positive 32-bit integer |
| Asset bytes | Positive and at most 536,870,912 bytes |
| List offset | Non-negative integer |
| List limit | 1–100; frontend default 50 |
| Search query | Trimmed; maximum 200 Unicode scalar values |
| Recovery title | 0-4,096 UTF-8 bytes; recovery only; no NUL |
| Recovery Markdown | 0-4,194,304 UTF-8 bytes; recovery only; no NUL |

The frontend may prevalidate for user feedback, but invalid backend input returns
a typed error and never writes files or rows.

## 7. Managed root and path contract

Schema v4 adds `newspaper_clipping_roots`. A `download_snapshot` root resolves
from the source job's persisted batch destination, never a frontend payload:

```text
<destination>/
└─ Newspaper snapshots/
   ├─ .linkvault/
   │  ├─ clipping-root-v1.json
   │  ├─ staging/<clipping-id>/clipping-v1.webp.part
   │  ├─ trash/<clipping-id>-<deletion-nonce>/clipping-v1.webp
   │  └─ quarantine/<timestamp>-<reason>-<name>/
   └─ <sanitized edition name - code>/
      └─ <publication-date>/
         └─ <clipping-id>/clipping-v1.webp
```

Derived thumbnails remain under the dedicated app-data cache. Existing v3
rows are assigned to `legacy-managed-v1`, which resolves the former
`LinkVaultData/newspaper-clippings` layout for read/recovery only.

### FR-ASSET-001: Backend-derived paths

The new canonical relative path is exactly:

```text
<sanitized edition name - code>/<publication-date>/<clipping-id>/clipping-v1.webp
```

React cannot override root, directory, filename, extension, asset version, or
relative path.

### FR-ASSET-002: Containment

Every managed read/write/delete operation:

1. Resolves the row's registered clipping root and verifies its marker.
2. Rejects absolute or parent-component relative paths.
3. Creates parent directories itself.
4. Uses `symlink_metadata` and rejects symlinks.
5. Canonicalizes existing targets and verifies containment before reading.
6. Never follows a user-created symlink out of the managed root.
7. Never recreates a registered root during read, recovery, or cleanup.
8. Never scans for or automatically rebinds a moved root; any future explicit
   reconnect flow must verify the root marker before updating its locator.

### FR-ASSET-003: File permissions and replacement

- Files are written with create-new semantics in staging.
- An existing canonical directory for a new clipping ID is treated as a
  collision/recovery condition, not overwritten.
- Asset promotion uses a same-volume atomic rename from reserved staging into
  the edition/date/clipping directory.
- No canonical file is modified in place in V1.

### FR-ROOT-RECONNECT-001: Settings list

The root service lists registry identity, kind, and backend-derived display
path from SQLite, plus any process-memory cached probe outcome. It does not add
stale availability columns to the root registry or synchronously probe every
filesystem path while listing. A root without a cached outcome begins as
`unchecked`; Settings renders it as `checking` while requesting a probe.
Availability probes run off the UI-sensitive thread, are coalesced per root,
and are concurrency bounded so a disconnected removable or network destination
cannot freeze Settings.

### FR-ROOT-RECONNECT-002: Check again

`Check again` accepts only a root ID. The backend resolves the stored locator,
rejects reparse/symlink substitution, and verifies the exact marker. It returns
`connected`, `offline`, or `marker_mismatch`; it does not create directories,
rewrite markers, scan other locations, or mutate the locator.

### FR-ROOT-RECONNECT-003: Reconnect

`Reconnect…` uses a backend-owned native folder-selection flow (or an opaque
selection token with equivalent ownership). React never submits an arbitrary
path to the repository. The selected directory must be the existing
`Newspaper snapshots` root and present the requested root ID in its marker.
After canonicalization and uniqueness checks, one serialized writer transaction
updates `locator`, `locator_key`, and `updated_at`. A failure leaves the old
locator and all notes/assets unchanged.

### FR-ROOT-RECONNECT-004: Offline behavior

Offline or mismatched roots do not block title/note search, editing, or list
metadata. Canonical images and thumbnails show the existing unavailable state.
Visible thumbnail requests coalesce the root probe so one result page cannot
perform one blocking offline-path check per row.

## 8. Idempotency contract

The frontend creates one UUID `operationId` when the user confirms a save. That
value is used as the clipping ID and remains constant for transport retries of
that one save attempt.

Conceptual request fragment:

```ts
interface CreateNewspaperClippingRequest {
  operationId: string;
  pageId: string;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
}
```

### FR-IDEMPOTENCY-001

If no row or staging state exists for the ID, normal creation begins.

### FR-IDEMPOTENCY-002

If a `ready` or `missing` row already exists for the ID, the command returns the
existing clipping summary and does not create another asset.

### FR-IDEMPOTENCY-003

If a `creating` row exists, the service runs recovery for that ID and returns
ready or a typed failure.

### FR-IDEMPOTENCY-004

If the ID belongs to `delete_pending`, create fails with
`CLIPPING_OPERATION_CONFLICT`.

A new explicit user save generates a new operation ID, even for identical
source geometry. V1 does not deduplicate content.

## 9. Creation state machine

Filesystem and SQLite cannot be committed atomically. Creation must therefore
be recoverable rather than pretending to be one transaction.

### CREATE-STATE-001: Prepare

Outside a database write transaction:

1. Validate operation ID and request.
2. Resolve and snapshot the source record.
3. Register/commit the verified root, then decode, crop, and encode to
   `.linkvault/staging/<id>/clipping-v1.webp.part`.
4. Flush/close the file.
5. Decode final bytes and validate dimensions.
6. Compute final byte count and SHA-256.
7. Rename `.webp.part` to a complete staging filename
   `.linkvault/staging/<id>/clipping-v1.webp`.

No SQLite row exists yet. A failure removes the current operation’s staging
directory when safe.

### CREATE-STATE-002: Register creating row

Through `DatabaseWriter`, insert one row with:

```text
asset_state = creating
asset_root_id = <registered download-snapshot root ID>
asset_relative_path = <edition>/<date>/<id>/clipping-v1.webp
asset_version = 1
revision = 1
note_markdown = ''
```

The closure performs SQL only and does not access image bytes or the
filesystem. A uniqueness conflict follows the idempotency contract.

### CREATE-STATE-003: Promote asset

Outside a database transaction, atomically rename:

```text
.linkvault/staging/<id>  →  <edition>/<date>/<id>
```

Before rename, reject an unexpected existing final directory. After rename,
verify the final regular file still matches size, dimensions, and checksum.

### CREATE-STATE-004: Mark ready

Through `DatabaseWriter`, update exactly the same row from `creating` to
`ready`, clear `asset_error_code`, and update `updated_at`. The update must be
conditional on ID and expected state.

### CREATE-STATE-005: Return

Return only after the row is `ready`. If the final writer response is lost, a
retry with the same ID resolves through the idempotency contract.

### Creation crash recovery table

| Crash/failure point | Durable state | Required recovery |
|---|---|---|
| Before staging file complete | `.part`, no row | Remove stale operation staging after grace period |
| Complete staging, before row insert | staging file, no row | Quarantine/remove stale orphan after grace period |
| After `creating` insert, before promotion | creating row + staging | Validate, promote, mark ready |
| After promotion, before ready update | creating row + canonical file | Validate and mark ready |
| Creating row, no staging or final file | creating row only | Mark missing with `ASSET_CREATION_INCOMPLETE` |
| Ready update succeeds, response lost | ready row + canonical file | Idempotent retry returns existing clipping |
| DB insert fails | staging only | Remove current staging; no clipping is visible |
| Promotion fails | creating row + staging | Return typed failure; startup/request recovery retries or marks missing |

### AC-PERSIST-001

No ordinary list query may expose a `creating` row as successfully saved.

### AC-PERSIST-002

For every simulated crash point above, restart recovery reaches exactly one of:

- A readable `ready` clipping with correct checksum.
- A preserved `missing` row with safe error code and intact note/provenance.
- No row and no managed canonical asset when creation never registered.

## 10. Update contract

Conceptual request:

```ts
interface UpdateNewspaperClippingRequest {
  clippingId: string;
  expectedRevision: number;
  title: string;
  noteMarkdown: string;
}
```

SQL semantics:

```sql
UPDATE newspaper_clippings
SET title = ?2,
    note_markdown = ?3,
    revision = revision + 1,
    updated_at = ?4
WHERE id = ?1
  AND revision = ?5
  AND asset_state IN ('ready', 'missing');
```

### FR-UPDATE-001

Validation occurs before writer submission. Failed validation does not alter
revision or updated time.

### FR-UPDATE-002

Exactly one changed row returns the updated clipping revision.

### FR-UPDATE-003

Zero changed rows triggers a follow-up read that distinguishes:

- `CLIPPING_NOT_FOUND`
- `CLIPPING_REVISION_CONFLICT`
- `CLIPPING_NOT_EDITABLE` for creating/delete-pending state

### FR-UPDATE-004

Asset version, geometry, provenance, source links, and asset metadata are
immutable through the note update command.

### FR-UPDATE-005

No-op updates whose normalized title and Markdown equal the stored values return
the current record without incrementing revision.

### FR-UPDATE-006: Atomic checkpoint acknowledgement

When a canonical update includes an acknowledged writer session and sequence,
the existing note savepoint deletes only that clipping's matching-session
checkpoint whose sequence is less than or equal to the submitted sequence. A
revision conflict, validation failure, SQL failure, or stale session/sequence
leaves the checkpoint intact. A no-op canonical update may clear it only after
proving the submitted title and Markdown equal the canonical bytes.

## 11. Read and list contract

### Repository detail read

A detail read returns `ready` and `missing` rows. It derives:

- Versioned canonical media URL when ready.
- Versioned thumbnail URL or ensure-thumbnail state.
- `source_available` by joining both source IDs to a completed page/job.
- Normalized crop geometry calculated from persisted source pixels.
- Safe asset state and error code.

It never returns absolute paths.

### Repository list query

Input:

```rust
pub struct NewspaperClippingListQuery {
    pub query: String,
    pub sort: NewspaperClippingSort,
    pub offset: u32,
    pub limit: u32,
}
```

Sort enum:

```text
updated_desc
created_desc
publication_desc
title_asc
```

Visible states:

```sql
asset_state IN ('ready', 'missing')
```

Search fields:

```text
title
note_markdown
edition_name_snapshot
edition_code_snapshot
publication_date_snapshot
page_number_snapshot
```

Search uses bound parameters and an explicit `ESCAPE` character. `%`, `_`, and
the escape character are escaped before the pattern is wrapped in `%...%`.

### FR-READ-001

List queries load only summary fields and a bounded plain-text excerpt. They do
not return the full Markdown body or canonical bytes.

### FR-READ-002

The excerpt is computed from at most the first 4,096 UTF-8 bytes of Markdown,
converted to plain text without executing HTML/MDX, collapsed to whitespace,
and truncated for display. The complete Markdown is fetched only by detail ID.

### FR-READ-003

Pagination is deterministic by including `id` as the final sort key.

## 12. Derived thumbnail cache

Thumbnail files are cache, not aggregate state.

### Thumbnail output

- Format: lossy WebP is allowed because the thumbnail is derived.
- Bounding box: 512×320 pixels.
- Aspect ratio: preserved.
- Upscaling: prohibited.
- Source: canonical clipping asset only.
- Cache schema version: integer constant starting at 1.
- Filename key includes clipping ID and canonical asset version.

### Thumbnail ensure flow

1. Confirm clipping row is `ready` and asset version matches.
2. Return existing valid cache file when present.
3. Coalesce concurrent requests for the same clipping.
4. Decode the canonical asset off the UI-sensitive thread.
5. Resize and encode to a `.part` file.
6. Validate, atomically promote, and return a versioned media URL.
7. If generation fails, the list retains a placeholder; canonical detail remains
   available.

No thumbnail table is required in V1. Paths are deterministically derived from
clipping ID, asset version, and cache schema version.

## 13. Media protocol extension

Conceptual variants:

```text
/clipping/<clipping-id>?v=<asset-version>
/clipping-thumbnail/<clipping-id>?v=<asset-version>-<cache-schema-version>
```

### FR-MEDIA-001

The canonical route queries the clipping row, requires `asset_state = ready`,
requires requested asset version equality, derives the expected managed path,
and validates file type, containment, MIME, non-empty bytes, size, and checksum
policy before responding.

### FR-MEDIA-002

The thumbnail route verifies the clipping and requested composite version,
derives the deterministic cache path, and serves only a regular contained file.
Before reading, metadata must report a positive byte count no greater than
8,388,608 bytes. The exact returned buffer must be a static, decodable WebP
whose width is 1 through 512 and height is 1 through 320. Animated/multiframe,
empty, malformed, oversized-byte, oversized-dimension, symlinked, reparse, and
stale-version files are rejected.

### FR-MEDIA-003

Success responses use private immutable caching keyed by version. Error
responses use `no-store`.

### FR-MEDIA-004

Malformed, stale, missing, unsupported, symlinked, or escaped requests return a
safe HTTP status/body without database values or paths.

### FR-MEDIA-005

A checksum mismatch on the canonical route records or schedules transition to
`missing`/integrity state through a service boundary; the protocol handler must
not perform an unbounded blocking database write while serving bytes.

## 14. Startup recovery

Recovery runs after application database initialization and before clipping
views claim ready state. It is divided into a bounded synchronous reconciliation
and deferred cleanup.

### RECOVERY-001: Creating rows

For each `creating` row, oldest first, bounded to all such rows because their
expected count is very small:

1. If canonical final file exists and validates, mark ready.
2. Else if complete staging file exists and validates, promote and mark ready.
3. Else mark missing with `ASSET_CREATION_INCOMPLETE`.
4. Never create a second row or change clipping ID.

### RECOVERY-002: Delete-pending rows

Complete the confirmed deletion:

1. Move canonical directory to trash when still under assets.
2. Delete the row through the writer.
3. Remove matching thumbnail cache best-effort and record a safe diagnostic on
   failure without recreating or retaining the row.
4. Remove trash directory after row deletion.

If a pre-row-deletion step fails, leave a retryable `delete_pending` row or
trash entry and record a safe diagnostic. Cache or trash cleanup failure after
row deletion never recreates the clipping; deferred managed cleanup may remove
the leftover.

### RECOVERY-003: Ready rows

Startup does not hash every ready asset, which would scale poorly. It verifies
existence and metadata lazily on detail/media access. A bounded sample may be
included in release diagnostics but not required on every launch.

### RECOVERY-004: Orphans

- Staging directories without a row older than 24 hours move to quarantine.
- Canonical asset directories without a row older than 24 hours move to
  quarantine rather than immediate deletion.
- Trash entries without a row older than 24 hours may be deleted.
- Exact-ID derived thumbnail files without a row older than 24 hours may be
  deleted. Malformed names, non-regular entries, symlinks, and reparse points
  are never deletion targets.
- Quarantine entries are retained for seven days, then eligible for deletion.
- Cleanup is submitted to a detached blocking task; application setup and the
  UI/runtime startup thread do not wait for enumeration.
- Each launch may completely enumerate the staging, assets, trash, quarantine,
  and clipping-thumbnail managed categories. Actual `ReadDir` items consumed
  are counted and reported honestly; enumeration/inspection is not described
  as bounded.
- Mutation attempts remain capped at 32 independently for each managed
  category per launch. Repeated launches remove later eligible entries as
  earlier successful mutations leave the directory.
- Traversal is streaming and does not sort or retain all names. No persisted
  filename cursor is used because directory order is unspecified and stable
  Rust provides no portable durable `ReadDir` position.
- Cleanup retains every containment and age rule above and must not scan user
  newspaper download directories.

The 500-entry and 5,000-entry managed-directory measurements record wall time
and approximate/peak process memory for the supported Windows environment.

### RECOVERY-005: Diagnostics

Recovery records only safe IDs, state, operation, elapsed time, and error class.
It does not log note content, absolute paths, image bytes, or raw SQL errors
that may include paths.

## 15. Delete state machine

Conceptual request:

```ts
interface DeleteNewspaperClippingRequest {
  clippingId: string;
  expectedRevision: number;
}
```

### DELETE-STATE-001: Mark intent

Through the writer, conditionally update a `ready` or `missing` row with the
matching revision to `delete_pending`. A conflict or missing row aborts before
filesystem mutation.

### DELETE-STATE-002: Move managed asset

Outside a database transaction:

- If the canonical directory exists and is validly contained, atomically rename
  it under `trash/<id>-<nonce>`.
- If it is already absent, continue; missing asset must not prevent deletion of
  the note after explicit confirmation.
- Remove derived thumbnails best-effort after the row deletion is durable, or
  defer removal to managed cleanup. Cache failure records a safe diagnostic and
  never blocks deletion of the title, note, row, or canonical aggregate.

### DELETE-STATE-003: Delete row

Through the writer, delete only the row still in `delete_pending` state.

### DELETE-STATE-004: Final cleanup

After row deletion, remove the trash directory. Failure is recorded for startup
cleanup and does not recreate the deleted clipping.

### AC-PERSIST-003

A crash at any delete step eventually yields either:

- The original readable clipping before delete intent was committed, or
- A completed deletion with no row and no durable canonical asset.

After `delete_pending` is committed, recovery completes deletion because the
user explicitly confirmed it.

## 16. Source-deletion behavior

### FR-SOURCE-001

Deleting a source page or its parent job sets clipping `source_page_id` and
`source_job_id` to null through foreign keys. It does not modify provenance,
geometry, title, note, asset metadata, asset state, revision, created time, or
updated time.

### FR-SOURCE-002

Because job deletion cascades into page deletion, tests must cover both
foreign-key transitions in one transaction and prove no clipping row is
removed.

### FR-SOURCE-003

Source availability is derived, not persisted as a mutable boolean.

## 17. World Journal reset behavior

The existing reset operation must be reviewed and amended so that:

- `newspaper_clippings` is excluded from delete statements.
- `newspaper_clipping_roots`, the legacy managed root, and registered snapshot
  roots are not removed.
- Clipping thumbnails remain or may be lazily regenerated; they are not part of
  the existing newspaper front-page thumbnail root.
- Source `SET NULL` actions complete under foreign keys.
- Reset diagnostics and copy state that clippings are preserved.

### AC-PERSIST-004

Given at least one ready clipping, one missing clipping, and source jobs/pages
for both

When Reset World Journal completes

Then clipping row count, titles, notes, revisions, checksums, canonical files,
and asset states are unchanged

And both source IDs are null

And source availability is false

And the database passes `foreign_key_check` and `quick_check`.

## 18. Error codes

Persistence and asset services use stable safe codes:

```text
CLIPPING_INVALID_ID
CLIPPING_INVALID_TITLE
CLIPPING_NOTE_TOO_LARGE
CLIPPING_INVALID_MARKDOWN
CLIPPING_NOT_FOUND
CLIPPING_NOT_EDITABLE
CLIPPING_REVISION_CONFLICT
CLIPPING_OPERATION_CONFLICT
CLIPPING_ASSET_ROOT_UNAVAILABLE
CLIPPING_ASSET_PATH_INVALID
CLIPPING_ASSET_COLLISION
CLIPPING_ASSET_WRITE_FAILED
CLIPPING_ASSET_PROMOTION_FAILED
CLIPPING_ASSET_VALIDATION_FAILED
CLIPPING_ASSET_MISSING
CLIPPING_ASSET_CHECKSUM_MISMATCH
CLIPPING_DATABASE_WRITE_FAILED
CLIPPING_DATABASE_READ_FAILED
CLIPPING_RECOVERY_FAILED
CLIPPING_DELETE_FAILED
```

The crop pipeline adds its own source and geometry codes in specification 03.
UI copy maps from safe codes; raw underlying error strings are diagnostic-only
and redacted.

## 19. Phase 1 acceptance criteria

### AC-PERSIST-005: Fresh schema

Given an empty database

When application initialization runs

Then the clipping table and indexes exist

And the current application schema version is recorded

And no empty migration backup is created.

### AC-PERSIST-006: Legacy migration

Given a populated database at the previous supported version

When initialization runs

Then a verified backup is created before schema mutation

And all representative data survives

And the clipping schema exists

And `foreign_key_check` and `quick_check` return clean results.

### AC-PERSIST-007: Writer ownership

Given production clipping source code

When the persistence structural gate scans it

Then every clipping insert, update, state transition, and delete uses
`DatabaseWriter`

And no file or image operation occurs inside a writer closure.

### AC-PERSIST-008: Managed containment

Given malformed IDs, absolute paths, parent components, symlinks, directories,
and out-of-root canonical targets

When asset or protocol operations run

Then each is rejected without reading, overwriting, or deleting an outside
file

And no absolute path reaches React.

### AC-PERSIST-009: Creation recovery

Given fixtures at every creation crash point

When startup recovery runs repeatedly

Then it is idempotent and reaches one valid terminal state defined in section 9.

### AC-PERSIST-010: Update conflict

Given revision 3 in SQLite

When two callers submit updates with expected revision 3

Then exactly one succeeds as revision 4

And the other receives `CLIPPING_REVISION_CONFLICT`

And the winner’s complete title and Markdown are stored without partial merge.

### AC-PERSIST-011: Reset preservation

Reset passes AC-PERSIST-004 with canonical files byte-for-byte unchanged.

### AC-PERSIST-012: Delete recovery

Given fixtures at every delete crash point

When recovery repeats

Then confirmed deletion completes without deleting the source edition or any
other clipping asset.

### AC-PERSIST-013: Media safety

Given valid, stale, malformed, missing, symlinked, corrupt, and escaped clipping
media requests

When the protocol handles them

Then only the valid current-version request returns bytes

And every error body remains path-free and non-cacheable.

## 20. Phase 1 exit gate

Phase 1 is complete only when:

- The Phase 1 implementation PR changes no reader selection or note editor UI.
- Schema version, backup, fresh/legacy/current/future-version tests pass.
- Creation/update/delete/recovery and reset-preservation tests pass.
- Media protocol tests pass.
- The persistence structural baseline is updated through its approved process,
  not weakened.
- `cargo fmt --check`, `cargo clippy --all-targets`, full Rust tests,
  `verify:architecture`, `verify:persistence`, production frontend build, and
  `verify:release` pass.
- The PR records managed-directory fixtures and confirms all temporary files
  remain inside test-owned temporary directories.
- The coding agent stops. Phase 2 may not begin in the same PR.
