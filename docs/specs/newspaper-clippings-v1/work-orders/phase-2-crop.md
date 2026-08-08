# Work order: Phase 2 deterministic native clipping crop

**Status:** Blocked draft

**Assigned branch:** `feat/newspaper-clippings-phase-2-crop`

**Stacked base:** `feat/newspaper-clippings-phase-1-persistence`

**Primary specification:** `../03-native-crop-pipeline.md`

**Execution contract:** `../08-coding-agent-execution-contract.md`

## Entry gate

Codex must not edit production code until all are true:

- Phase 1 PR is approved and merged to `main`.
- The Rust formatting-baseline prerequisite is merged.
- This branch is rebased or recreated from the resulting current `main`.
- Phase 1 migration, managed-asset, recovery, protocol, and reset gates are
  green.
- The master PRD marks Phase 2 Ready.
- No Proposed decision blocks Phase 2.

A stacked draft PR exists now only to preserve the work queue and review scope.
It is not authorization to bypass the entry gate.

## Objective

Implement the backend-only source-resolution crop pipeline and callable Tauri
command that turns one validated normalized rectangle into one durable clipping
aggregate using the Phase 1 repository and managed-asset lifecycle.

Phase 2 ends with a deterministic, tested, release-measured backend command. It
does not add the reader Clip button, pointer overlay, Clippings library, note
editor, source-navigation UI, OCR, AI, tags, annotations, or release version.

## Mandatory reading order

1. `docs/architecture/README.md`
2. `docs/architecture/adr-001-unified-workflow-modular-monolith.md`
3. `docs/architecture/adr-002-newspaper-clippings-managed-assets.md`
4. `docs/specs/newspaper-clippings-v1/README.md`
5. `docs/specs/newspaper-clippings-v1/00-decision-register.md`
6. `docs/specs/newspaper-clippings-v1/02-domain-persistence-and-assets.md`
7. `docs/specs/newspaper-clippings-v1/03-native-crop-pipeline.md`
8. `docs/specs/newspaper-clippings-v1/07-verification-performance-and-release.md`
9. `docs/specs/newspaper-clippings-v1/08-coding-agent-execution-contract.md`
10. The merged Phase 1 implementation and its final PR evidence.

## Required preflight response

Before editing, update the PR body with:

- Exact current `main` and branch SHAs.
- Confirmation that Phase 1 and the formatting prerequisite are merged.
- Requirement/acceptance-criterion IDs assigned to Phase 2.
- Orientation strategy selected under `FR-SOURCE-004` and why it matches the
  current reader.
- Exact expected files and deliberately untouched files.
- Existing dependencies reused and any proposed dependency change.
- Resource-limit and concurrency design.
- Test fixture plan.
- Release-build crop baseline plan.
- Exit-gate commands and rollback boundary.

Stop if the orientation policy, decoder limits, stable-read identity, or
shutdown contract is ambiguous after reading the current code.

## Required implementation scope

### Command and DTO boundary

Add the asynchronous `create_newspaper_clipping` command and camelCase request/
response DTOs exactly as approved. The response contains IDs, provenance,
versioned media URL, dimensions, byte count, revision, and timestamps; it never
contains an absolute or relative filesystem path.

The Tauri command must remain a thin adapter into the Newspaper clipping
service.

### Validation

Implement and test:

- canonical operation UUID;
- page ID ASCII/length contract;
- positive expected media version;
- finite normalized values;
- `NORMALIZED_EPSILON = 0.000001` validation order;
- clamp only floating error within epsilon;
- minimum 32×32 source-pixel crop;
- checked arithmetic and all source/output limits.

### Authoritative source resolver

Resolve provenance only from SQLite and registered Newspaper state. Join the
page, job, and edition projection required by the specification.

Candidate order is binding:

1. valid retained original;
2. otherwise valid optimized image;
3. otherwise typed unavailable failure.

Validate regular non-symlink source files, registered output-root containment,
positive bounded size, supported extension/sniff/decode agreement, static image
input, and stable pre/post read metadata. Security failures must not silently
fall through to another candidate.

### Orientation and dimensions

Implement one approved `FR-SOURCE-004` strategy and tests. Do not ignore EXIF
orientation. Decoded oriented dimensions are authoritative. Handle stored/
decoded and original/display dimension mismatch exactly as specified.

### Pure geometry

Create a pure helper implementing exactly:

```text
left   = floor(clamp(x) × W)
top    = floor(clamp(y) × H)
right  = ceil(clamp(x + width) × W)
bottom = ceil(clamp(y + height) × H)
```

Persist `left`, `top`, `right-left`, and `bottom-top`. Do not independently
round width/height. Cover full-page, edge, reverse-invalid, epsilon, very large,
and randomized invariant cases.

### Crop and lossless WebP

Crop the oriented decoded raster without resizing, tone, sharpening, denoise,
threshold, or other reader effects. Preserve alpha. Use explicit lossless WebP
encoding. Validate final format/dimensions, size, checksum, and exact decoded
pixel equality in deterministic tests.

Write only to the operation-owned Phase 1 staging path and use the Phase 1
register/promote/ready state machine.

### Media-version recheck

Immediately before the creating row is inserted, re-read page status, media
version, and source paths. Remove the current staging operation and return the
approved stale/not-ready code on mismatch. Do not silently bind to changed
media.

### Execution and shutdown

- One concurrent full crop operation in V1.
- Acquire the crop permit before reading source bytes.
- Repeat idempotency state check after acquiring the permit.
- Run file read, decode, crop, encode, checksum, and validation on a bounded
  blocking path, not the WebView-sensitive async command path.
- Hold no database write transaction during image work.
- Reach a recoverable tracked state during cooperative shutdown.

### Idempotency

Before expensive work and again after acquiring the permit:

- ready/missing → return existing clipping;
- creating → targeted recovery;
- delete_pending → operation conflict;
- absent → continue.

SQLite remains authoritative across restart. An optional process-local in-flight
map may only coalesce identical operation IDs.

### Safe error contract

Add the approved crop/source/limit/encode/service codes without raw path, SQL,
decoder, or note leakage. UI copy is not Phase 2.

## Expected ownership/files

Likely changes are limited to:

```text
apps/desktop/src-tauri/src/providers/newspaper/
  clipping_models.rs
  clipping_repository.rs
  clipping_assets.rs
  clipping_service.rs
  commands.rs
  media_protocol.rs             only when response URL helpers require it
  source/crop modules added beneath this provider
  mod.rs
apps/desktop/src-tauri/src/lib.rs
apps/desktop/src/components/newspaper/newspaper-api.ts
apps/desktop/scripts/           clipping verification/baseline only when required
apps/desktop/package.json       script alias only when required by spec
package.json                    forwarding alias only when repository convention requires it
docs/performance/ or approved evidence path
```

Do not modify `NewspaperReader.tsx`, `NewspaperLibrary.tsx`, `App.tsx`, editor
dependencies, general notes code, LinkedIn, Coursera, release versions, or visual
styles in this phase.

## Required tests

Use generated, test-owned fixtures only. Include at minimum:

- finite/bounds/epsilon geometry table tests;
- randomized geometry invariant tests;
- full-page and all-edge crops;
- too-small crop rejection;
- JPEG, alpha PNG, and existing WebP sources;
- thin high-frequency text/grid pattern;
- corrupt/truncated/unsupported/mislabelled source;
- animated/multi-frame rejection where the decoder exposes it;
- source symlink, path escape, directory, empty, too-large, dimension, and pixel
  limit failures;
- EXIF orientation strategy case;
- source mutation during read;
- original-first and missing-original optimized fallback;
- security-invalid original does not fall through;
- original/display dimension mismatch;
- stale media version at initial lookup and final recheck;
- one-crop concurrency bound and duplicate operation coalescing;
- idempotent ready/creating/delete-pending states;
- shutdown before acceptance and recoverable shutdown after start;
- lossless decoded pixel equality;
- no raw paths in responses/errors;
- no file/database leak outside temporary roots.

## Performance evidence

Record a release-build crop baseline using generated small, representative, and
limit-near fixtures. Include source dimensions/bytes, crop dimensions, selected
source kind, queue wait, read/decode/crop/encode/validation/persistence elapsed
times, peak/process memory evidence available to the project, output bytes, and
hardware/toolchain details. Do not invent a final threshold; Phase 6 owns final
budgets.

## Exit gate

Run every Phase 2 command required by specification 07, including at minimum:

```powershell
cargo fmt --manifest-path apps\desktop\src-tauri\Cargo.toml --check
cargo clippy --manifest-path apps\desktop\src-tauri\Cargo.toml --all-targets
cargo test --manifest-path apps\desktop\src-tauri\Cargo.toml
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:persistence
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:newspaper-performance
npm.cmd --prefix apps\desktop run verify:newspaper-performance-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
npm.cmd --prefix apps\desktop run verify:release
git diff --check
```

If a clipping verification script does not yet exist but Phase 2 requirements
need it, add the smallest approved structural/data gate without claiming reader
or editor behavior.

## Codex start prompt

```text
Work only on the draft Phase 2 PR branch
`feat/newspaper-clippings-phase-2-crop`.

Do not edit production code until PR #2 and the rustfmt-baseline prerequisite
are merged, this branch is rebased on current main, and the master PRD marks
Phase 2 Ready. Read the documents in this work order's mandatory order.

Before coding, update the PR body with entry-gate proof, requirement IDs,
orientation strategy, expected files, tests, performance evidence plan, and
exit commands. Then implement only the deterministic backend crop/source
pipeline and thin create command defined by specification 03. Do not add reader
selection UI, Clippings library/editor, source navigation/delete UI, OCR, AI,
tags, annotations, release changes, or unrelated refactors.

Run every Phase 2 gate, record intermediate failures and fixes, update the PR
with exact evidence, and stop. Do not merge and do not begin Phase 3.
```
