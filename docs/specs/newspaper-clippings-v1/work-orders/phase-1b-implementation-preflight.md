# Phase 1B implementation preflight

**Assigned phase:** Phase 1B retrieval/reconnect foundation

**Working branch:** `codex/newspaper-clippings-search-reconnect`

## Entry gate

- Current `origin/main` (`d306789`) is an ancestor of this branch.
- Phase 1A implementation (`95b6059`) is the next ancestor and its focused
  clipping baseline passes, but it is not yet merged to `origin/main`.
- The approved Phase 1B plan (`da55a72`) is the current branch base.
- Therefore local stacked implementation is authorized by the product owner,
  but Phase 1B must not be published as independently mergeable before Phase
  1A lands.

## Authority read

- Architecture README, ADR-001, and ADR-002.
- Newspaper Clippings master PRD and complete decision register.
- Domain/persistence/assets specification 02.
- Verification/release specification 07.
- Coding-agent execution contract 08.
- Approved Phase 1B search/reconnect work order.
- Existing migration, clipping repository/model/service/root, command,
  composition-root, persistence verifier, and focused test owners.

## Approved decisions

- D-019 ranked local FTS and bounded Possible matches.
- D-032 download-destination snapshot roots.
- D-033 marker-verified root status/reconnect.
- One/two-character queries exclude Note; Note begins at three characters;
  fuzzy suggestions begin at four and are capped at 25.

## Implementation decisions

- `unicode-normalization` was reviewed through Context7 and the live Cargo
  registry, then added as a direct dependency. The resolved `0.1.25` release is
  MIT OR Apache-2.0, requires Rust 1.36, and provides Unicode Annex 15
  compatibility normalization.
- Empirical SQLite FTS5 trigram proof showed that raw external-content terms do
  not equate ASCII/full-width or NFC/decomposed spellings. Normalizing only the
  query therefore cannot satisfy normalized candidate recall.
- The product owner approved the second derived normalized candidate index.
  The implementation keeps the required raw external-content FTS index, adds
  a contentless normalized FTS index, and stores only normalized Title/Edition
  metadata for exact/prefix and short-query matching. It does not duplicate
  full canonical Note bodies or rewrite their bytes.

## Expected files

- `apps/desktop/src-tauri/src/app/database.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/storage.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_models.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_repository.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_roots.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/clipping_service.rs`
- `apps/desktop/src-tauri/src/providers/newspaper/commands.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- Existing colocated tests and persistence structural baseline only when the
  reviewed production SQL count changes.
- Phase 1B work-order/evidence corrections discovered during implementation.

`Cargo.toml` and `Cargo.lock` contain the reviewed Unicode-normalization
dependency needed by the approved search contract.

## Files excluded

- React, Settings UI, Clippings takeover UI, Reader interaction, crop service,
  Tiptap production wiring, release version, installer, and unrelated provider
  code.

## Proof to add

- Fresh/populated-v4/older/future/failure schema-v5 migration and backup proof.
- FTS5 availability, table/trigger/rebuild/parity/repair tests.
- Literal query, ranked English/Chinese, short-query, paging, fuzzy bound, and
  safe snippet tests.
- Root list/check/reconnect/open positive, offline, mismatch, duplicate,
  reparse, writer-failure, coalescing, and outside-sentinel tests.
- Exact command/service safe-error and no-path-input assertions.

The approved bounded trigram candidate source makes Possible matches best
effort rather than exhaustive at the four-scalar minimum. A middle edit or
transposition in a four-scalar term can share no trigram with the stored value.
Closing that recall gap would require an additional bigram candidate index (and
its measured storage/write cost) or raising the fuzzy minimum; neither is being
silently added to this phase.

The normalized FTS index is contentless so it does not duplicate canonical Note
bodies. Consequently a normalized-only Note match can be factual without the
raw external-content FTS being able to locate original-text offsets. The safe
bounded fallback returns the Note tag plus an unhighlighted plain-text excerpt;
it never invents a highlight or scans complete Notes per keystroke. Guaranteed
compatibility-normalized highlighting would require a separately approved
position index or duplicated normalized content. The product owner approved
the bounded unhighlighted fallback on 2026-08-09.

## Exit gates

- Focused clipping tests first.
- Architecture, persistence, UI, Newspaper performance, and clipping structural
  gates.
- Rust format, clippy, and full tests.
- Production audit/build and repository release gate.

## Rollback boundary

Schema v5 is forward-only after user data is written. Preserve the verified v4
backup and roll forward; never run destructive downgrade SQL. The FTS objects
are derived and may be rebuilt without changing canonical clipping rows. Root
reconnect failures leave the old locator and every note/asset unchanged.

## Baseline

- First Visual Studio baseline invocation failed before Cargo because nested
  `cmd.exe` quoting split the DevCmd path.
- Corrected invocation: format plus focused `clipping_` tests passed, 59/59,
  zero ignored, in 107.3 seconds including a cold 94-second compile.
- During integration, the expanded focused matrix exposed a stale verified-root
  cache misclassifying an offline snapshot root as a missing clipping asset.
  The media failure path now bypasses the cache before scheduling an integrity
  transition; the final optimized focused matrix passes 84/84 with zero
  ignored.
- The first corrected old-schema audit exposed that the test fixtures had left
  v5 normalized search objects behind while claiming to represent v2/v3/v4.
  True older-schema fixtures now remove every derived v5 search object. The
  forced-failure fixture was changed from a self-healable partial table to a
  conflicting view so rollback and retry are still proven.
- Release measurement on Windows, 500 clippings: 250.583 ms to seed the derived
  indexes in one test transaction, 43.923 ms for the first 50-row page, and
  43.897 ms for the deep 50-row page. The bounded Possible-matches request
  against an exact 2,097,152-byte Note took 55.750 ms, considered at most 100
  candidates, and returned one bounded result.
- Final exact-source `verify:release` passed: architecture/persistence/UI/build,
  800/800 serialized baseline writes, and 502 release tests passed with only
  the four documented pre-existing ignores. Production npm audit reports zero
  vulnerabilities; the full development audit retains the pre-existing Vite
  -> PostCSS -> `nanoid@3.3.16` advisory and this phase does not rewrite the
  unrelated JavaScript lockfile.
