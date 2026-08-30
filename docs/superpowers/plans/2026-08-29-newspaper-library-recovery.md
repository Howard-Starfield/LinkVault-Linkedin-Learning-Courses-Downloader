# Newspaper library recovery (one-button) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Settings action recovers Newspaper edition pages and snapshot clippings (webp + `note.md`) from a save-to folder, with idempotent dedupe and no separate snapshot Recover CTA.

**Architecture:** Add `recover_newspaper_library` that resolves the picked path to a save-to root + optional `Newspaper snapshots` tree, then (1) extends `archive_service` to scan `{name} - CODE/YYYY-MM-DD/*.webp` editions, (2) registers/reconnects the clipping root via existing root APIs, (3) walks clipping dirs and inserts ready rows through `DatabaseWriter`. Frontend Settings calls only this command; Repair stays maintenance-only.

**Tech Stack:** Rust/Tauri 2 newspaper provider, rusqlite via `DatabaseWriter`, React Settings in `App.tsx` + `NewspaperSnapshotRootsSettings.tsx`, existing `clipping_assets` / `archive_service` / `clipping_repository`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-29-newspaper-library-recovery-design.md`
- Do not call `archive_service::repair` from recover (no redundant JPG deletion).
- Skip trees named `Newspaper snapshots` and `Youtubes` during edition scan.
- Clipping writes only via `DatabaseWriter`; blocking FS/image work on `spawn_blocking`.
- No production `unwrap`/`expect` on command paths; no TypeScript `any`.
- Do not overwrite existing clipping notes/titles on re-import.
- Preserve unrelated dirty `apps/desktop/src-tauri/src/app/managed_process.rs`.
- Do not bump release version in this work unless the user asks.

---

## File map

| File | Responsibility |
| --- | --- |
| `apps/desktop/src-tauri/src/providers/newspaper/models.rs` | `RecoverNewspaperLibraryResult` (+ nested count/status types) |
| `apps/desktop/src-tauri/src/providers/newspaper/archive_service.rs` | Edition scan for current layout; richer import counts; keep legacy flat names |
| `apps/desktop/src-tauri/src/providers/newspaper/library_recovery.rs` | **Create:** path resolve, snapshot root ensure, clipping walk + import orchestration |
| `apps/desktop/src-tauri/src/providers/newspaper/clipping_repository.rs` | `insert_recovered_ready` (ready + note + FTS in one write) |
| `apps/desktop/src-tauri/src/providers/newspaper/clipping_service.rs` | Thin `import_from_snapshot_disk` / ensure-root helpers used by recovery |
| `apps/desktop/src-tauri/src/providers/newspaper/commands.rs` | `recover_newspaper_library` command; deprecate UI use of archive-only import |
| `apps/desktop/src-tauri/src/lib.rs` | Register new command |
| `apps/desktop/src-tauri/src/providers/newspaper/mod.rs` | `mod library_recovery;` |
| `apps/desktop/src-tauri/src/providers/newspaper/tests.rs` | Integration-style fixtures for recover |
| `apps/desktop/src/components/newspaper/newspaper-api.ts` | Typed `recoverNewspaperLibrary` invoke |
| `apps/desktop/src/App.tsx` | One Recover button + toast from structured result; Repair hint |
| `apps/desktop/src/components/newspaper/NewspaperSnapshotRootsSettings.tsx` | Remove Recover snapshot CTA; keep Open/Check |

---

### Task 1: Result types

**Files:**
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/models.rs`
- Modify: existing tests in `models.rs` only if serde round-trip needed (optional)

**Interfaces:**
- Produces:
  - `RecoverNewspaperLibraryResult` (camelCase serde)
  - `RecoverSnapshotRootStatus` enum: `Registered`, `Reconnected`, `AlreadyConnected`, `Missing`, `MarkerMismatch`, `Unavailable`

- [ ] **Step 1: Add types**

Append to `models.rs` near `RepairNewspaperLibraryResult`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoverNewspaperLibraryResult {
    pub editions_imported: u32,
    pub editions_already_known: u32,
    pub editions_skipped: u32,
    pub clippings_imported: u32,
    pub clippings_already_known: u32,
    pub clippings_skipped: u32,
    pub snapshot_root: RecoverSnapshotRootStatus,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecoverSnapshotRootStatus {
    Registered,
    Reconnected,
    AlreadyConnected,
    Missing,
    MarkerMismatch,
    Unavailable,
}
```

- [ ] **Step 2: Commit**

```powershell
git add apps/desktop/src-tauri/src/providers/newspaper/models.rs
git commit -m "feat(newspaper): add recover library result types"
```

---

### Task 2: Edition scan for `{name} - CODE/date/A01.webp` (TDD)

**Files:**
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/archive_service.rs`
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/tests.rs` (or tests module inside `archive_service` if preferred; prefer `tests.rs` to match existing import tests)

**Interfaces:**
- Consumes: existing `archive_identity`, catalog rows in `newspaper_editions`
- Produces: `pub(super) fn import(...) -> Result<EditionImportCounts, String>` where

```rust
pub struct EditionImportCounts {
    pub imported: usize,
    pub already_known: usize,
    pub skipped: usize,
}
```

Change `import` return from `usize` to `EditionImportCounts` (update all call sites / tests). Keep internal batch insert behavior; count `changes()==0` as already_known.

- [ ] **Step 1: Write failing test for current layout**

In `tests.rs`, add (use `tempdir`, seed DB with `storage::initialize`, write a 4×4 webp under `波士頓 - BO/2026-08-09/A01.webp`, also create empty `Newspaper snapshots` and `Youtubes` dirs):

```rust
#[test]
fn archive_import_recognizes_edition_folder_layout() {
    // arrange fixture as above with BO catalog code present after initialize
    let counts = archive_service::import(&db_path, &archive).unwrap();
    assert_eq!(counts.imported, 1);
    assert_eq!(counts.already_known, 0);
    let again = archive_service::import(&db_path, &archive).unwrap();
    assert_eq!(again.imported, 0);
    assert_eq!(again.already_known, 1);
}
```

Also assert snapshots/Youtubes files are not imported as editions.

- [ ] **Step 2: Run test — expect FAIL**

```powershell
npm run cargo:test -- --lib providers::newspaper::tests::archive_import_recognizes_edition_folder_layout
```

Expected: FAIL (0 imported) or compile error if return type not updated yet.

- [ ] **Step 3: Implement scan**

In `archive_service.rs`:

1. Add `EditionImportCounts`.
2. Before/alongside the recursive legacy file walk, scan **immediate** children of root:
   - Skip if name equals `SNAPSHOT_DIRECTORY_NAME` or `Youtubes` (case-insensitive for snapshots; exact/ASCII for Youtubes).
   - Parse code with: take `file_name`, find last `" - "`, right side must be len 2 ASCII uppercase.
   - Confirm `EXISTS (SELECT 1 FROM newspaper_editions WHERE code=? AND publication_date='')` **after** opening connection (or pre-load known codes once connection is open — prefer load codes first, then scan FS, then import).
3. For each date child `YYYY-MM-DD`, collect image files (jpg/jpeg/png/webp) in that directory only (not recursive into deeper unrelated trees).
4. Feed groups into the same insert loop as today; track skipped for immediate children that look like dirs but fail parse/catalog.
5. Preserve legacy `archive_identity` walk for flat files under root/date folders **excluding** the snapshot tree (already skipped).

Helper sketch:

```rust
fn parse_edition_folder_name(name: &str) -> Option<(String, String)> {
    let (left, code) = name.rsplit_once(" - ")?;
    if code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()) {
        Some((left.to_string(), code.to_string()))
    } else {
        None
    }
}
```

- [ ] **Step 4: Update existing import tests** for `EditionImportCounts` (`.imported` instead of bare usize).

- [ ] **Step 5: Run tests — expect PASS**

```powershell
npm run cargo:test -- --lib providers::newspaper::tests::archive_import
```

- [ ] **Step 6: Commit**

```powershell
git add apps/desktop/src-tauri/src/providers/newspaper/archive_service.rs apps/desktop/src-tauri/src/providers/newspaper/tests.rs apps/desktop/src-tauri/src/providers/newspaper/commands.rs
git commit -m "fix(newspaper): import edition folders from save-to layout"
```

---

### Task 3: `insert_recovered_ready` repository helper (TDD)

**Files:**
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/clipping_repository.rs`
- Test: add unit test in `clipping_repository.rs` `#[cfg(test)]` module or `clipping_service.rs` tests — prefer a focused test next to repository if harness is heavy; otherwise add service-level test in Task 5.

**Interfaces:**
- Produces:

```rust
pub fn insert_recovered_ready(
    connection: &Connection,
    record: &NewClippingRecord,
    note_markdown: &str,
) -> Result<()>
```

Inserts with `asset_state = 'ready'`, `asset_version = 1`, `note_markdown = ?`, FTS includes note. Same columns as `insert_creating` but ready + note.

- [ ] **Step 1: Write failing test** (minimal DB init used elsewhere in clipping tests — reuse pattern from `clipping_service` tests that open temp DB + `storage::initialize`).

Assert row `asset_state = ready`, `note_markdown` equals input, FTS searchable.

- [ ] **Step 2: Run — FAIL** (function missing)

- [ ] **Step 3: Implement** by copying `insert_creating` SQL and changing:
  - `'creating'` → `'ready'`
  - `''` note placeholder → bound `note_markdown`
  - pass note into `insert_normalized_search_document`

- [ ] **Step 4: Run — PASS**

- [ ] **Step 5: Commit**

```powershell
git commit -m "feat(newspaper): insert recovered clippings as ready with notes"
```

---

### Task 4: Snapshot clipping walk + parse helpers

**Files:**
- Create: `apps/desktop/src-tauri/src/providers/newspaper/library_recovery.rs`
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/mod.rs` (`mod library_recovery;`)

**Interfaces:**
- Produces pure helpers (unit-tested without DB):

```rust
pub(super) struct DiscoveredClipping {
    pub id: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub asset_relative_path: String, // forward-slash path under snapshot root
    pub absolute_webp: PathBuf,
    pub note_markdown: String,
}

pub(super) fn parse_clipping_dir_name(name: &str) -> Option<(String /*page or empty*/, String /*id*/)>
pub(super) fn discover_clippings(snapshot_root: &Path) -> Result<Vec<DiscoveredClipping>, String>
```

Parsing rules:
- Dir name is UUID → page `""` then default page label `"Page"` or `"A01"` only if needed for validation — prefer page from `Page X - uuid` prefix; if bare UUID use `"Page"` sanitized or `"clipping"` — check `validate_page_number`; if empty invalid, use `"A01"` as last resort **only if** validate requires non-empty. Read `validate_page_number` and match production.
- Relative path = path under snapshot root with `\` → `/`.
- Skip `.linkvault`, skip top-level `assets` directory name when walking from snapshot root.
- Skip dirs without `clipping-v1.webp`.
- Read `note.md` if present (UTF-8 lossy or strict UTF-8; on invalid UTF-8 skip note with warning later). Empty file → `""`.

- [ ] **Step 1: Failing unit tests** for `parse_clipping_dir_name` and a tiny temp tree discover.

- [ ] **Step 2: Implement helpers**

- [ ] **Step 3: Tests PASS**

- [ ] **Step 4: Commit**

```powershell
git commit -m "feat(newspaper): discover snapshot clippings on disk"
```

---

### Task 5: Wire clipping import + root ensure into `ClippingService`

**Files:**
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/clipping_service.rs`
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/library_recovery.rs`

**Interfaces:**
- Consumes: `register_download_destination`, `reconnect` / load-by-marker, `insert_recovered_ready`, `discover_clippings`, `validate_note_markdown`, `ClippingAssetLayout::validate_relative_path_for_id`, image load for dimensions/checksum
- Produces:

```rust
impl ClippingService {
    pub fn ensure_snapshot_root_for_destination(
        &self,
        save_to_root: &Path,
        now: i64,
    ) -> Result<(ClippingRoot, RecoverSnapshotRootStatus), ClippingError>;

    pub fn import_discovered_clippings(
        &self,
        root: &ClippingRoot,
        discovered: &[DiscoveredClipping],
        now: i64,
    ) -> Result<(u32 /*imported*/, u32 /*known*/, u32 /*skipped*/, Vec<String> /*warnings*/), ClippingError>;
}
```

Behavior:
- `ensure_snapshot_root_for_destination`: call `register_download_destination(save_to_root, now)`. If marker `root_id` already in DB with different locator but selected verifies, reconnect. Map outcomes to `RecoverSnapshotRootStatus`.
- Import: for each discovery, if `load_by_id` Some → already_known (no overwrite). Else build `NewClippingRecord` with `source_* = None`, `source_kind = Optimized` (webp), mime `image/webp`, crop full image, title = page number (or edition+page), checksum sha256 of file bytes. Insert via writer + `insert_recovered_ready`. On validate failure → skipped + warning.

- [ ] **Step 1: Integration test** with temp save-to + snapshots marker + one clipping folder with note.md

- [ ] **Step 2: Implement service methods**

- [ ] **Step 3: PASS + commit**

```powershell
git commit -m "feat(newspaper): import disk clippings into clipping root"
```

---

### Task 6: `recover_newspaper_library` command

**Files:**
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/library_recovery.rs` — `pub fn recover(db_path, clipping_service, path) -> Result<RecoverNewspaperLibraryResult, String>`
- Modify: `apps/desktop/src-tauri/src/providers/newspaper/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` invoke handler list

**Interfaces:**
- Path resolve:
  - If pick is dir named `Newspaper snapshots` → `snapshot = pick`, `save_to = parent`
  - Else `save_to = pick`, `snapshot = pick.join("Newspaper snapshots")` if exists
- Hard error if `save_to` not a safe directory
- Call `archive_service::import(db_path, save_to)` → edition counts (warnings on Err? prefer Err propagates for unusable root)
- If snapshot missing → `snapshot_root = Missing`, clipping counts 0
- Else ensure root + discover + import; clipping failures append warnings, still return Ok with partial counts
- Emit `library_events::after_archive_change`; also refresh clipping UI if there is an existing invalidate/list event — reuse whatever create-clipping uses (search `newspaper://` emits). If none, archive event + frontend refresh list on toast is enough when Settings closes/reopens; prefer emit existing clipping invalidate broadcast if available.

Command:

```rust
#[tauri::command]
pub async fn recover_newspaper_library(
    app: tauri::AppHandle,
    state: State<'_, NewspaperState>,
    clipping: State<'_, ClippingService>,
    path: String,
) -> Result<RecoverNewspaperLibraryResult, String>
```

Keep `import_existing_newspaper_archive` as thin wrapper returning `editions_imported` only **or** remove from `lib.rs` + frontend if unused after Task 7. Prefer remove from frontend and leave command registered one release for compatibility **or** delete both if no external callers — grep and delete if only App.tsx.

- [ ] **Step 1: Implement recover + command + register**

- [ ] **Step 2: End-to-end Rust test** `recover_newspaper_library_imports_editions_and_clippings` in `tests.rs` using service harness

- [ ] **Step 3: Commit**

```powershell
git commit -m "feat(newspaper): recover library command for editions and clippings"
```

---

### Task 7: Settings UI — one button

**Files:**
- Modify: `apps/desktop/src/components/newspaper/newspaper-api.ts`
- Modify: `apps/desktop/src/App.tsx`
- Modify: `apps/desktop/src/components/newspaper/NewspaperSnapshotRootsSettings.tsx`

**Interfaces:**
- TS type mirroring Rust camelCase result
- `recoverNewspaperLibrary(path: string)` → invoke `recover_newspaper_library`

- [ ] **Step 1: Add API helper**

```ts
export type RecoverNewspaperLibraryResult = {
  editionsImported: number;
  editionsAlreadyKnown: number;
  editionsSkipped: number;
  clippingsImported: number;
  clippingsAlreadyKnown: number;
  clippingsSkipped: number;
  snapshotRoot: "registered" | "reconnected" | "alreadyConnected" | "missing" | "markerMismatch" | "unavailable";
  warnings: string[];
};

export function recoverNewspaperLibrary(path: string) {
  return invoke<RecoverNewspaperLibraryResult>("recover_newspaper_library", { path });
}
```

- [ ] **Step 2: Replace Settings recover button**

- Label: **Recover newspaper library**
- Picker title: same
- Hint under buttons: “Choose your Newspaper download folder. Imports editions and clippings from Newspaper snapshots.”
- Repair button: keep; add short hint “Maintenance: rename legacy pages / optimize / remove redundant JPGs.”
- Toast from counts (editions + clippings + already known); include warning count if any
- After success, call `listNewspaperSnapshotRoots` refresh if Settings snapshot section is open (export a callback or rely on `NewspaperSnapshotRootsSettings` `open` remount — simplest: pass a `refreshToken` state incremented on recover)

- [ ] **Step 3: Remove Recover snapshot folder button** from `NewspaperSnapshotRootsSettings.tsx` (keep Open + Check). Update subtitle/hint: recovery is via Recover newspaper library above.

- [ ] **Step 4: Remove unused `reconnectNewspaperSnapshotRoot` UI import if unused; keep command registered for offline emergency (optional). Spec says remove CTA — keep Rust command.

- [ ] **Step 5: `npm run verify:no-any` and `npm run build`**

- [ ] **Step 6: Commit**

```powershell
git commit -m "feat(newspaper): unify Settings recover for editions and clippings"
```

---

### Task 8: Verification gate

- [ ] **Step 1: Run**

```powershell
npm run verify:no-any
npm run build
npm run cargo:test -- --lib providers::newspaper
npm run cargo:clippy
```

- [ ] **Step 2: Manual smoke** (if Tauri available): pick `C:\Users\howard\Downloads\Ai_script\Newpaper` on empty/new DB — expect non-zero editions and clippings; second run already known; notes visible for LA clippings that have `note.md`.

- [ ] **Step 3: Final commit** only if verification fixed nits

---

## Spec coverage checklist

| Spec requirement | Task |
| --- | --- |
| One Recover newspaper library button | 7 |
| Import current edition folder layout | 2 |
| Keep legacy flat archive names | 2 |
| Import clippings webp + note.md | 4–5 |
| Register/reconnect snapshot root | 5–6 |
| Idempotent dedupe | 2, 5 |
| Remove separate snapshot Recover CTA | 7 |
| Repair unchanged / not inside recover | 6–7 |
| Structured toast counts | 1, 6–7 |
| Path safety / DatabaseWriter / spawn_blocking | 5–6 |
| Tests with fixture tree | 2, 4, 5, 6 |

## Placeholder / consistency notes

- Serde enum variants: Rust `AlreadyConnected` ↔ TS `"alreadyConnected"` via `rename_all = "camelCase"`.
- Relative paths must use `/` and pass `validate_relative_path_for_id`.
- Clipping IDs must be lowercase UUID (`validate_clipping_id`).
- Do not invent `source_job_id` for recovered clippings.
