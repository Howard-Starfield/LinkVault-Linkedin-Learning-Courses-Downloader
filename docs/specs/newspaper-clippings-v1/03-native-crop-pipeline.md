# Newspaper Clippings V1: deterministic native crop pipeline

**Status:** Approved

**Primary implementation phase:** Phase 2

**Related decisions:** D-003 through D-009, D-014, D-017, D-022, D-030

## 1. Purpose

This specification defines the request/response contract and deterministic
algorithm that converts a reader selection into a source-resolution clipping.
It covers source lookup, media-version checks, path and image validation,
normalized coordinate handling, exact integer rounding, crop encoding,
checksums, bounded execution, idempotency, error codes, and tests.

Phase 2 may expose a callable Tauri command and backend tests, but it must not
add the reader selection overlay or production Clippings editor UI.

## 2. Command contract

Conceptual Tauri command:

```text
create_newspaper_clipping
```

Rust request shape:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperClippingRequest {
    pub operation_id: String,
    pub page_id: String,
    pub expected_media_version: i64,
    pub rect: NormalizedCropRect,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedCropRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
```

TypeScript request shape:

```ts
export type NormalizedCropRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type CreateNewspaperClippingRequest = {
  operationId: string;
  pageId: string;
  expectedMediaVersion: number;
  rect: NormalizedCropRect;
};
```

Response shape:

```ts
export type CreateNewspaperClippingResponse = {
  clippingId: string;
  title: string;
  editionCode: string;
  editionName: string;
  publicationDate: string;
  pageNumber: string;
  imageUrl: string;
  assetVersion: number;
  assetWidth: number;
  assetHeight: number;
  assetByteCount: number;
  revision: number;
  createdAt: number;
};
```

The response contains no absolute or relative filesystem path.

## 3. Request validation

Validation occurs before source-file reads or staging-directory creation.

### FR-CROP-001: Operation ID

- Must satisfy the canonical ID contract in specification 02.
- Must be accepted idempotently as defined there.
- Must not contain `/`, `\`, `.`, percent escapes, NUL, or Unicode lookalikes.

### FR-CROP-002: Page ID

- Non-empty.
- At most 200 ASCII characters.
- Only ASCII alphanumeric, hyphen, and underscore.
- The page must be resolved from SQLite; it is never interpreted as a path.

### FR-CROP-003: Expected media version

- Positive integer.
- Must equal the current `newspaper_pages.media_version` at initial source
  resolution and immediately before the creating row is inserted.

### FR-CROP-004: Finite rectangle

Every value must satisfy `Number.isFinite` semantics. NaN, positive/negative
infinity, and non-numeric serialization fail with `INVALID_CROP_RECT`.

### FR-CROP-005: Normalized bounds

Use tolerance:

```text
NORMALIZED_EPSILON = 0.000001
```

Validation order:

1. Reject `x < -epsilon` or `y < -epsilon`.
2. Reject `width <= 0` or `height <= 0`.
3. Reject `x > 1 + epsilon` or `y > 1 + epsilon`.
4. Reject `x + width > 1 + epsilon`.
5. Reject `y + height > 1 + epsilon`.
6. After passing, clamp each boundary into `[0, 1]` only to absorb floating
   error within epsilon.

Do not silently repair materially out-of-range rectangles.

## 4. Authoritative source record

The service performs a read-only database query that joins the page, job, and
edition/catalog projection needed to derive provenance. It must obtain at least:

```text
page.id
page.job_id
page.page_number
page.status
page.original_path
page.optimized_path
page.pixel_width
page.pixel_height
page.media_version
page.checksum
job.edition_code
job.publication_date
job.output_dir
resolved edition display name
```

### FR-SOURCE-001: Eligibility

The source page must:

- Exist.
- Belong to an existing Newspaper job.
- Have status `completed`.
- Have a positive media version.
- Resolve a non-empty edition code, edition display name, publication date, and
  page number.

Otherwise return a typed source error before creating managed files.

### FR-SOURCE-002: Source candidate order

Candidate order is:

1. `original_path` when present and valid.
2. `optimized_path` when present and valid.
3. No candidate → `SOURCE_MEDIA_UNAVAILABLE`.

“Valid” requires all of:

- The path comes from the registered database row, not the frontend.
- It is absolute after normal path resolution.
- `symlink_metadata` identifies a regular, non-symlink file.
- Canonical path remains beneath the canonical registered job output directory.
- File length is positive and within the maximum source-byte limit.
- Extension and sniffed/decoded format are supported.
- The image decodes under configured resource limits.

A present but invalid original does not automatically fall through when the
failure indicates a security problem such as symlink or path escape. Security
failures abort. A missing/deleted ordinary original may fall through to a valid
optimized image.

### FR-SOURCE-003: Supported inputs

V1 supports registered JPEG, PNG, and WebP source pages. Format is determined
by decode/sniffing and must agree with a supported extension/MIME mapping.
Animated WebP and multi-frame input are rejected; a newspaper page is one
static raster.

### FR-SOURCE-004: Source orientation

The saved rectangle must map to the orientation shown by the reader. V1 must
not silently crop a different raw orientation because of EXIF metadata.

The Phase 2 implementation must choose and test one of these compliant
strategies:

1. Detect and apply JPEG EXIF orientation before geometry and crop, or
2. Detect non-identity orientation and reject with
   `SOURCE_ORIENTATION_UNSUPPORTED`, while the reader uses the same raw
   orientation policy.

Ignoring orientation metadata without detection is non-compliant.

### FR-SOURCE-005: Stable read

The service records file metadata before and after reading all source bytes.
If length or modification identity changes during the read, it returns
`SOURCE_MEDIA_CHANGED_DURING_READ`. The decoded source checksum is calculated
from the stable byte buffer and stored as a provenance snapshot.

## 5. Resource limits

The pipeline uses explicit limits before allocating decoded image buffers.

```text
MAX_SOURCE_FILE_BYTES = 1,073,741,824        # 1 GiB
MAX_SOURCE_DIMENSION  = 32,768 pixels
MAX_SOURCE_PIXELS     = 80,000,000 pixels
MAX_OUTPUT_BYTES      = 536,870,912          # 512 MiB
MIN_CROP_WIDTH        = 32 pixels
MIN_CROP_HEIGHT       = 32 pixels
MAX_CONCURRENT_CROPS  = 1
```

### FR-LIMIT-001

Width × height uses checked integer multiplication. Overflow is a typed limit
failure, not a panic.

### FR-LIMIT-002

The decoder is configured with memory/dimension limits where supported and
also validates decoded dimensions before materializing avoidable copies.

### FR-LIMIT-003

A source outside the limit fails with `SOURCE_MEDIA_TOO_LARGE`. The UI may
explain that the page exceeds the safe clipping limit; it must not recommend
raising an internal constant without measured review.

### FR-LIMIT-004

Only one crop operation may decode/encode at a time in V1. Waiting callers do
not hold database transactions and can report queued/running state through the
service boundary.

## 6. Decoded source dimensions

The dimensions returned by the decoder are authoritative for coordinate
conversion and persistence.

### FR-DIM-001

If stored page dimensions are present and differ from decoded dimensions, the
service records a safe diagnostic. It may continue only when:

- The media version still matches.
- Orientation handling is consistent.
- The normalized rectangle remains valid.
- The difference is not caused by a changed/partial file.

The clipping stores actual decoded dimensions, not stale metadata.

### FR-DIM-002

If the current optimized display image and retained original have different
oriented dimensions, the original cannot be used silently. Use the displayed
optimized source or fail with `SOURCE_DIMENSION_MISMATCH`. The existing
optimizer is expected to preserve dimensions, and this branch is a defensive
regression guard.

## 7. Exact normalized-to-pixel algorithm

After clamping only within epsilon, define normalized boundaries:

```text
left_n   = clamp(x, 0, 1)
top_n    = clamp(y, 0, 1)
right_n  = clamp(x + width, 0, 1)
bottom_n = clamp(y + height, 0, 1)
```

For decoded dimensions `W` and `H`, use `f64` intermediates:

```text
left   = floor(left_n   × W)
top    = floor(top_n    × H)
right  = ceil (right_n  × W)
bottom = ceil (bottom_n × H)
```

Then clamp integer edges defensively:

```text
left   ∈ [0, W]
top    ∈ [0, H]
right  ∈ [left, W]
bottom ∈ [top, H]
```

Persist:

```text
crop_x      = left
crop_y      = top
crop_width  = right - left
crop_height = bottom - top
```

### FR-GEOMETRY-001

The algorithm above is binding. Do not use independent rounding of `width × W`
or `height × H`, because it can create a one-pixel gap at the far edge.

### FR-GEOMETRY-002

After conversion:

- `crop_width >= 32`
- `crop_height >= 32`
- `crop_x + crop_width <= W`
- `crop_y + crop_height <= H`

Otherwise return `CROP_TOO_SMALL` or `INVALID_CROP_RECT` as appropriate.

### Geometry examples

For `W=1000`, `H=2000`:

```text
rect = { x: 0.1, y: 0.2, width: 0.25, height: 0.1 }
left   = floor(100) = 100
top    = floor(400) = 400
right  = ceil(350)  = 350
bottom = ceil(600)  = 600
result = x 100, y 400, width 250, height 200
```

Full page:

```text
rect = { x: 0, y: 0, width: 1, height: 1 }
result = x 0, y 0, width W, height H
```

Tiny floating error:

```text
x + width = 1.0000004
```

is accepted and clamped because it is within epsilon. `1.00001` is rejected.

## 8. Crop extraction

### FR-EXTRACT-001

Crop the already oriented decoded image at the exact integer rectangle. Do not
resize, sharpen, denoise, recolor, threshold, or apply reader tone.

### FR-EXTRACT-002

Preserve alpha when input pixels contain alpha. Opaque input may be encoded as
opaque RGB. Output pixel values, after decoding the lossless WebP, must equal
the corresponding decoded source-region pixels for supported color channels.

### FR-EXTRACT-003

Color-profile metadata and EXIF metadata need not be copied in V1. The decoded
raster is canonical. The implementation must document any color conversion
performed by the image library and include a representative fixture.

## 9. Lossless WebP encoding

### FR-ENCODE-001

Use an encoder API/configuration that explicitly selects lossless WebP. A
numeric quality setting without lossless mode is prohibited.

### FR-ENCODE-002

Write encoded bytes only to the operation-owned staging `.part` path.

### FR-ENCODE-003

Before completing staging:

1. Reject empty output.
2. Reject output over `MAX_OUTPUT_BYTES`.
3. Decode encoded bytes.
4. Confirm output format is WebP.
5. Confirm output dimensions exactly equal crop dimensions.
6. Compare decoded output pixels with the decoded crop for deterministic test
   fixtures.
7. Compute lowercase SHA-256 over exact final bytes.

Production may not compare every pixel a second time after encoding because
that duplicates memory work; exact pixel comparison is mandatory in tests.
Production validation must at least decode and verify dimensions/format.

### FR-ENCODE-004

After validation, close the file and atomically rename:

```text
clipping-v1.webp.part → clipping-v1.webp
```

inside the staging operation directory. Promotion into the canonical assets
root follows specification 02.

## 10. Media-version recheck

Immediately before the `creating` row is inserted, perform a fresh read of:

```text
page status
page media_version
page original_path
page optimized_path
```

### FR-VERSION-001

If status is no longer completed, return `SOURCE_PAGE_NOT_READY` and remove the
current staging operation.

### FR-VERSION-002

If media version differs from the request, return `SOURCE_MEDIA_STALE` and
remove current staging. Do not create a row.

### FR-VERSION-003

If the selected registered source path changed after stable read, return
`SOURCE_MEDIA_STALE` unless the exact stable bytes still correspond to the
same registered original candidate and the decision is proven safe by tests.
The default is rejection rather than silent rebinding.

## 11. Title and provenance creation

The backend derives all provenance from the source record. The frontend cannot
supply edition, date, page, title, dimensions, or source kind.

Default title:

```text
<edition display name> · <publication YYYY-MM-DD> · <page number>
```

The service normalizes only surrounding whitespace and validates the title
limits from specification 02. It does not translate names, infer an article
headline, or call OCR/AI.

Persisted source checksum is the SHA-256 of the stable source byte buffer used
to decode the crop. It is a provenance snapshot and does not replace the page’s
existing checksum semantics.

## 12. Execution boundary

### FR-EXEC-001

The public Tauri command is asynchronous and delegates CPU/blocking work to a
bounded blocking execution path. Reading a large file, decoding, cropping,
encoding, checksum calculation, and file validation must not execute on the
WebView/main-thread-sensitive command path.

### FR-EXEC-002

Acquire the one-crop semaphore before source-byte read and release it after
staging cleanup or canonical promotion completes. Do not hold the permit while
waiting for user interaction.

### FR-EXEC-003

Do not open a database write transaction before acquiring the permit or during
image work. Read-only source queries use ordinary runtime connections; row
insertion/state updates use `DatabaseWriter` only at the boundaries defined in
specification 02.

### FR-EXEC-004

If application shutdown begins before a request is accepted, return
`CLIPPING_SERVICE_UNAVAILABLE`. If a request has begun canonical creation,
cooperative shutdown waits for it to reach a recoverable filesystem/database
state rather than abandoning an untracked partial operation.

## 13. Idempotent invocation

### FR-IDEM-CROP-001

Before expensive work, query clipping state by operation ID:

- `ready` or `missing`: return existing detail/summary.
- `creating`: invoke targeted recovery and return its result.
- `delete_pending`: return operation conflict.
- absent: continue.

### FR-IDEM-CROP-002

After acquiring the crop permit, repeat the absent-state check to prevent two
concurrent invokes with the same ID from both decoding.

### FR-IDEM-CROP-003

A process-local in-flight map may coalesce identical operation IDs, but SQLite
state remains authoritative across restart.

## 14. Error contract

Safe codes added by the crop service:

```text
INVALID_CROP_RECT
CROP_TOO_SMALL
SOURCE_PAGE_NOT_FOUND
SOURCE_PAGE_NOT_READY
SOURCE_MEDIA_STALE
SOURCE_MEDIA_UNAVAILABLE
SOURCE_MEDIA_PATH_INVALID
SOURCE_MEDIA_UNSUPPORTED
SOURCE_MEDIA_TOO_LARGE
SOURCE_MEDIA_DECODE_FAILED
SOURCE_MEDIA_CHANGED_DURING_READ
SOURCE_ORIENTATION_UNSUPPORTED
SOURCE_DIMENSION_MISMATCH
SOURCE_CROP_FAILED
CLIPPING_ENCODE_FAILED
CLIPPING_OUTPUT_TOO_LARGE
CLIPPING_OUTPUT_VALIDATION_FAILED
CLIPPING_SERVICE_UNAVAILABLE
```

Conceptual failure response:

```ts
export type CreateNewspaperClippingFailure = {
  code: string;
  safeMessage: string;
  retryable: boolean;
  operationId: string;
};
```

### Retry classification

| Code | Retryable without changing request? |
|---|---|
| `SOURCE_MEDIA_STALE` | No; refresh manifest/version, then explicit resubmit |
| `SOURCE_PAGE_NOT_READY` | Maybe after page state changes |
| `SOURCE_MEDIA_CHANGED_DURING_READ` | Yes after a short delay |
| `CLIPPING_SERVICE_UNAVAILABLE` | Yes after app/service is available |
| `CLIPPING_ENCODE_FAILED` | Yes once; repeated failure requires diagnostics |
| `INVALID_CROP_RECT` | No; correct selection/request |
| `CROP_TOO_SMALL` | No; select a larger region |
| `SOURCE_MEDIA_PATH_INVALID` | No automatic retry; security/integrity issue |
| `SOURCE_ORIENTATION_UNSUPPORTED` | No; requires supported source handling |

Raw decoder, encoder, IO, SQL, and path strings are not returned to React.

## 15. Pure geometry module

The normalized validation and pixel conversion must be implemented as pure
functions independent of Tauri, SQL, filesystem, and image decoding so they can
receive exhaustive unit and property-style tests.

Conceptual contract:

```rust
pub fn validate_normalized_rect(
    rect: NormalizedCropRect,
) -> Result<ValidatedNormalizedCropRect, CropGeometryError>;

pub fn to_source_pixels(
    rect: ValidatedNormalizedCropRect,
    source_width: u32,
    source_height: u32,
) -> Result<SourcePixelCropRect, CropGeometryError>;
```

No test may duplicate a different rounding algorithm in production helper code.

## 16. Required test fixtures

Create isolated generated fixtures inside temporary directories:

1. **Grid PNG:** Every pixel encodes its x/y location, allowing exact crop
   comparison.
2. **Opaque JPEG:** Deterministic decoded crop compared to lossless output.
3. **Alpha PNG:** Transparent and semi-transparent pixels.
4. **Lossless WebP:** Existing WebP source.
5. **High-frequency text-like pattern:** Thin black/white strokes that expose
   lossy artifacts.
6. **Dimension-boundary image:** Small image for edge and full-page cases.
7. **Corrupt/truncated file.**
8. **Unsupported static file with image extension.**
9. **Symlink and out-of-root registered path fixtures.**
10. **Non-identity orientation JPEG fixture or equivalent orientation test.**
11. **Changing file fixture:** Metadata/bytes mutate between pre/post checks.
12. **Large-dimension header/decode-bomb fixture:** Rejected before unsafe
    allocation where the decoder permits.

No test fixture writes outside its test-owned temporary directory.

## 17. Geometry test matrix

At minimum:

- Full page.
- Top-left 10%.
- Bottom-right rectangle ending exactly at 1.0.
- Rectangle beginning/ending on fractional pixel boundaries.
- Reverse drag normalized by frontend produces same request as forward drag.
- Values within epsilon.
- Values beyond epsilon.
- Zero/negative width or height.
- NaN and infinities through direct Rust construction.
- Minimum exactly 32×32.
- One pixel below minimum.
- Width/height 1.
- Checked multiplication overflow/limit path.
- 10,000 deterministic pseudo-random valid rectangles proving:

  ```text
  0 <= x < x + width <= source_width
  0 <= y < y + height <= source_height
  ```

- Adjacent normalized rectangles share the expected integer boundary without a
  gap caused by independent width rounding.

## 18. Pixel correctness tests

### AC-CROP-001: Exact lossless region

Given the generated grid image

When a clipping is created for a known rectangle

Then decoded canonical output dimensions equal the integer crop dimensions

And every output pixel equals the corresponding decoded source pixel.

### AC-CROP-002: No reader effects

Given the same normalized request associated with reader tones original, soft,
dim, and inverted in frontend fixtures

When the backend creates clippings

Then canonical bytes/checksum are identical because tone is not part of the
request or pipeline.

### AC-CROP-003: Zoom independence

Given equivalent normalized rectangles calculated at reader zooms 50%, 100%,
120%, and 300%

When the backend crops each using the same operation-independent source

Then their source-pixel geometry and decoded output pixels are identical.

### AC-CROP-004: Source priority

Given both valid original and optimized files with distinguishable pixels

When creating a clipping

Then the original is used and provenance records `original`.

Given a missing ordinary original and valid optimized file

Then optimized is used and provenance records `optimized`.

Given a symlinked/out-of-root original

Then the request fails securely rather than falling through silently.

### AC-CROP-005: Media stale

Given request media version 4 and current page version 5

When creation runs

Then it returns `SOURCE_MEDIA_STALE`

And no clipping row, canonical asset, or retained staging operation exists.

### AC-CROP-006: Stable read

Given a source that changes during read

Then creation returns `SOURCE_MEDIA_CHANGED_DURING_READ`

And no row is committed from mixed bytes.

### AC-CROP-007: Idempotency

Given two concurrent calls with the same operation ID

When both resolve

Then exactly one canonical asset directory and one clipping row exist

And both successful responses refer to the same clipping ID.

### AC-CROP-008: Bounded execution

Given two different valid crop requests

When they run concurrently

Then at most one full decode/encode section is active

And neither holds a database write transaction while waiting for the permit or
processing image bytes.

### AC-CROP-009: Error hygiene

Given every failure fixture

Then the frontend-visible result contains a stable safe code and message

And contains no absolute path, raw SQL, stack trace, source URL credential, or
image bytes.

## 19. Performance evidence

Phase 2 records a release-build baseline rather than inventing a permanent
latency threshold before measurement. The report includes:

- Machine/CPU/RAM and build commit.
- Source format, dimensions, file bytes, and decoded pixels.
- Crop dimensions and output bytes.
- Queue wait, source read, decode, crop, encode, validation, filesystem,
  database-register, promotion, and ready-update durations.
- Peak process working-set delta where practical.
- Confirmation that the UI/main thread performed no decode/encode work.
- One and two-request scenarios proving the semaphore bound.

The report is committed under:

```text
docs/performance/newspaper-clippings-crop-windows-<date>.json
```

Phase 6 may ratify blocking thresholds from this evidence. Functional and
security correctness are blocking in Phase 2 even before latency budgets are
ratified.

## 20. Phase 2 exit gate

Phase 2 is complete only when:

- Phase 1 is merged and green.
- The deterministic command/service is callable without production reader UI.
- All geometry, source, orientation, pixel, idempotency, recovery handoff,
  security, and failure tests pass.
- Lossless output is proven by exact pixel fixtures, not inferred from file
  extension or encoder quality.
- Crop concurrency is structurally and dynamically bounded to one.
- No database transaction spans image work.
- The release-build crop baseline is committed.
- Existing architecture, persistence, Newspaper performance, Rust, frontend
  build, and release gates remain green.
- The coding agent stops. Reader interaction belongs to Phase 3.
