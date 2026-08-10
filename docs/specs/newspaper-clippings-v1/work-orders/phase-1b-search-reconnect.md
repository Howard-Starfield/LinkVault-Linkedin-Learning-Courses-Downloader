# Phase 1B work order: ranked search and snapshot reconnect foundation

**Status:** Approved for implementation after Phase 1A lands

**Branch:** `codex/newspaper-clippings-search-reconnect`

**Depends on:** D-019, D-032, D-033, Phase 1A storage amendment

**Exit:** Backend/schema foundation only. Stop before production Settings,
Clippings takeover UI, Reader, crop, or Tiptap wiring.

## 1. Objective

Provide the durable schema and provider-owned services required for:

1. relevance-ranked local clipping search with factual field matches;
2. a separately requested, bounded Possible matches result set; and
3. marker-verified status/check/reconnect operations for automatically created
   snapshot roots.

The nested filesystem is never searched for note content. SQLite remains the
title/Markdown source of truth; FTS and root probe state are derived.

## 2. Expected owners

- `app/database.rs`: schema version 5, verified backup/migration transaction.
- `providers/newspaper/storage.rs`: FTS table, triggers, rebuild/parity checks.
- `providers/newspaper/clipping_models.rs`: literal query normalization,
  match-field/search response/root-status types and hard limits.
- `providers/newspaper/clipping_repository.rs`: confident search, candidate
  retrieval, deterministic ordering, index repair and locator update SQL.
- `providers/newspaper/clipping_service.rs`: orchestration, bounded fuzzy
  scoring, root probe coalescing and reconnect policy.
- `providers/newspaper/clipping_roots.rs`: marker verification and reconnect
  containment; no dialog/UI ownership.
- thin Newspaper Tauri command adapter: root-ID actions and Rust-owned native
  folder selection through the already-installed `tauri-plugin-dialog`
  `DialogExt`/`pick_folder` API.
- persistence/architecture verification scripts and dated evidence only when a
  measured baseline is required.

Do not hand-edit generated bindings. Regenerate them only through the existing
repository command after command/type changes are final.

## 3. Schema v5

Create external-content `newspaper_clippings_fts` with trigram-tokenized:

- `title`
- `note_markdown`
- `edition_name_snapshot`
- `edition_code_snapshot`

Use the clipping table's SQLite `rowid` as `content_rowid`. Add insert, delete,
and update triggers. Installation order is:

1. verify bundled `ENABLE_FTS5`;
2. take the standard verified pre-migration backup;
3. create the FTS table and all triggers in the migration transaction;
4. issue the external-content `rebuild` command;
5. prove representative row/content parity and `quick_check`;
6. write schema version 5 only after every provider migration succeeds.

Any failure rolls back. Index repair drops/recreates/rebuilds only derived FTS
objects. It never updates or deletes canonical clipping rows.

## 4. Confident search contract

Input is normalized/trimmed, at most 200 Unicode scalar values, and always
bound as literal data. The command must not expose FTS query grammar.

Search all five fields:

- Title
- Note
- Edition name/code
- Date
- Page

Date and Page use literal matching only. For text fields, rank:

1. exact normalized title;
2. normalized title prefix;
3. weighted FTS score (Title > Note > Edition);
4. literal Date/Page contribution below text matches;
5. `updated_at DESC`;
6. clipping ID.

Freeze numeric FTS weights only after a committed mixed English/Chinese golden
fixture proves expected order. Return no score to React. Return cumulative
`matchedFields`, safe bounded plain-text snippet parts (`text` plus
`highlighted`, never HTML or byte offsets), total count, and a deterministic
50-row page. Title/Edition/Date/Page branches accept one Unicode scalar value;
the Note branch requires three and must not run for shorter queries. The
response returns authoritative `noteSearchApplied` so the UI can state that
notes were excluded without inventing a Note tag or snippet.

## 5. Possible matches contract

This is a separate request issued only after all confident pages are exhausted.

- Minimum query length: four Unicode scalar values.
- Fields: Title, Note, Edition only.
- Candidate source: bounded trigram candidate query.
- Candidate cap before scoring: explicit constant, initially measured with 100.
- Note work: bounded match windows/snippets only; never every complete note.
- Similarity: documented Unicode-normalized Damerau-Levenshtein or equivalent
  deterministic edit-distance rule.
- Exclude every confident result ID.
- Return at most 25 unique rows.
- Return `possibleMatch = true` and factual matched fields; no percentage.
- Never fuzzy Date or Page.

The golden fixture includes English typos, Chinese substitutions, mixed-width
characters, short queries, unrelated near strings, and structured Date/Page
counterexamples.

## 6. Root status and reconnect contract

Repository/service operations accept a root ID, not a frontend path.

### List

Return registry identity/kind and a backend-provided display path without
probing every root synchronously. Include a process-memory cached outcome when
available; otherwise return `unchecked` and let Settings render `checking`
while it requests the bounded probe. Do not add a persistently stale status
column. React may display the path but cannot feed it back into file operations.

### Check again

Resolve the stored locator off the UI-sensitive thread and coalesce concurrent
checks per root. Return:

- `connected`
- `offline`
- `marker_mismatch`

Do not create, scan, mutate, or mark clipping rows missing.

### Reconnect

The Tauri boundary owns native folder selection or an equivalent opaque token.
The service requires the existing `Newspaper snapshots` directory, validates
ordinary-directory containment and the exact root marker, canonicalizes it,
rejects locator ownership conflicts, then updates locator/key/timestamp through
the serialized writer. No marker/file is created or rewritten. Failure leaves
the old row untouched.

### Open folder

Accept root ID only. Reverify the marker immediately before opening. A stale,
offline, or mismatched root is not opened.

## 7. Performance and concurrency bounds

- Confident page: 50.
- Possible matches: 25 total.
- Candidate/window cap: explicit and release-measured.
- One current plus one near-future confident page at the frontend boundary.
- Root checks coalesced by root ID and concurrency bounded.
- Thumbnail/media requests share a short-lived verified-root result so a
  visible page on one offline drive cannot trigger one blocking probe per row.
- No full-note fuzzy scan per keystroke.
- No FTS or filesystem work inside a long serialized-writer transaction beyond
  the required row/index mutation itself.

## 8. Required proof

### Migration/index

- Fresh v5, populated v4, older supported schema, future-schema rejection.
- Backup exists and verifies before v5 mutation.
- Trigger synchronization for insert/update/delete and conflict/no-op paths.
- Forced rebuild/trigger failure rollback.
- Repair restores results without changing note bytes/revisions.
- `foreign_key_check`, `quick_check`, index parity, representative reads.

### Ranking/fuzzy

- Literal `%`, `_`, backslash, quotes, FTS operators, punctuation.
- One- and two-scalar queries match only Title/Edition/Date/Page; three scalars
  enable Note, including exact helper-copy and no-short-query-note-scan proof.
- Exact/prefix/weighted/tie ordering from golden fixture.
- English/Chinese and Unicode normalization cases.
- Accurate cumulative match fields and bounded safe snippets.
- Date/Page exact-only counterexamples.
- 0/1/50/51/500 confident rows, stable paging and dedupe.
- 0/1/25/over-25 candidates, confident exclusion, short-query suppression.
- Maximum-size note proves bounded fuzzy candidate/window work.

### Roots

- Connected/offline/mismatch status.
- Same-path retry and moved-path reconnect.
- Wrong/empty marker, duplicate locator, reparse/junction, unavailable network
  path, writer failure, and concurrent requests.
- Notes/search remain available offline; rows never become missing from a root
  probe.
- Sentinel outside selected root is byte-identical after every failure case.

### Gates

Run focused tests first, then architecture, persistence, UI contract, Newspaper
performance, format, clippy, full Rust, production audit/build, and the release
gate required by the repository. Record intermediate failures and exact reruns.

## 9. Existing-owner conflict audit

- The current Settings dialog is mounted from `apps/desktop/src/App.tsx`.
  Phase 1B does not edit that UI. Phase 4B should add a focused
  `NewspaperSnapshotLocations` component and mount it in the existing Newspaper
  Settings section; it must not create a second settings route or fold root
  status into the unrelated `saveSettings` payload.
- Existing `open_newspaper_download_folder(path)` actions open replaceable
  download-job folders and remain unchanged. Snapshot-root Open/Reconnect must
  use the new root-ID commands and must not reuse that path-authorized command.
- Existing React uses `@tauri-apps/plugin-dialog` for choosing new download
  destinations. Reconnect is different: Rust receives the selected
  `FilePath` from `tauri_plugin_dialog::DialogExt::file().pick_folder`, verifies
  the existing marker, and returns only the typed outcome/root summary. The
  selected reconnect path never becomes a React command argument.
- The root registry currently persists identity, kind, locator, locator key,
  and timestamps only. Probe status remains a bounded process-memory cache plus
  `unchecked`; do not add a stale status column or change clipping asset state
  during Settings checks.
- `newspaper-api.ts` remains the frontend IPC owner. Generated bindings, if
  enabled by the repository, are regenerated only through their generator.

## 10. Rollback

Do not downgrade a user database after schema v5 data exists. Roll forward with
a fix or explicitly restore the verified backup with a clear post-backup data-
loss warning. Disabling the UI does not authorize deleting the FTS index, root
registry, notes, or assets. A derived index may be rebuilt at any time from
canonical rows.

## 11. Phase 4B handoff

Phase 4B owns:

- Clippings toolbar and full-width search takeover;
- lazy confident paging and separated Possible matches UI;
- factual match tags and safe highlights;
- query/scroll/focus restoration and dirty-editor navigation guard;
- Settings Snapshot locations rows and Connected/Offline/Mismatch/Checking UI;
- Open folder, Check again, and Reconnect actions;
- production Tiptap list/detail/editor integration.

Phase 4B must consume these services; it must not create a second search index,
filesystem scanner, root store, or fuzzy implementation in React.
