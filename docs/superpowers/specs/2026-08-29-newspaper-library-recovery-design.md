# Newspaper library recovery (one-button) — design

**Date:** 2026-08-29  
**Status:** Approved for implementation planning  
**Product:** LinkedVault desktop (Tauri / newspaper provider)

## Problem

After an empty or replaced `LinkVaultData` database, users still have on-disk Newspaper downloads under a save-to folder such as `C:\Users\howard\Downloads\Ai_script\Newpaper`. Settings offered three separate recover/repair surfaces that did not match that reality:

1. **Recover newspaper archive** only recognized a legacy flat filename layout (`NY_20260809_A01.png`). The live downloader writes `{name_zh} - {CODE}/{YYYY-MM-DD}/A01.webp`, so picking the real folder reported **0 recovered**.
2. **Recover snapshot folder** only **reconnected** an existing DB root locator to a moved `Newspaper snapshots` directory. It did not scan disk or insert clipping rows (image + `note.md`) into an empty DB.
3. **Repair existing** renames legacy `.php` pages, runs optimize jobs, and removes redundant source JPGs. It is not import/dedupe and must not be confused with recovery.

## Goals

- One Settings primary action: **Recover newspaper library**.
- User picks the Newspaper **download / save-to parent** (or, if they pick nested `Newspaper snapshots`, resolve to that tree and treat parent as destination when possible).
- Import **edition page jobs** from the current on-disk layout (and keep legacy flat-name support).
- Import **clippings** from `Newspaper snapshots` (`clipping-v1.webp` + optional `note.md`) into SQLite as ready clippings.
- Register or reconnect the snapshot root from the on-disk `.linkvault/clipping-root-v1.json` marker when present.
- Idempotent: re-running recover skips already-known editions and clippings (dedupe in recover, not via Repair).
- Reorganize Settings so recovery is not split across archive + snapshot-root Recover buttons.

## Non-goals

- Auto-crawl at app startup.
- YouTube folder recovery inside this button.
- Changing clipping on-disk layout or note editor format.
- Making **Repair existing** part of the recover command (no silent JPG deletion / full optimize pass inside recover).
- Inventing LinkedIn-style `local:` URLs for newspaper pages.

## Real on-disk contracts (observed)

Save-to parent example: `…\Newpaper`

```text
Newpaper/
  波士頓 - BO/
    2026-08-09/
      .complete
      A01.webp … Axx.webp
  大華府 - DC/…
  洛杉磯 - LA/…
  Newspaper snapshots/
    .linkvault/clipping-root-v1.json   # {"schema_version":1,"root_id":"clipping-root-…"}
    assets/                            # ignore for import walk (managed assets)
    波士頓 - BO/
      2026-08-09/
        {uuid}/clipping-v1.webp
        Page B02 - {uuid}/clipping-v1.webp
        Page B02 - {uuid}/note.md      # optional; may be empty
  Youtubes/                            # ignore
```

Edition codes in folder names match catalog codes (`BO`, `DC`, `LA`, …). Clipping directory name is either bare UUID or `Page {page} - {uuid}`.

## Settings UX

### Newspaper library section

- **Primary:** one button **Recover newspaper library**  
  - Folder picker title matches the button.  
  - Hint: choose the Newspaper download folder; imports editions and clippings under `Newspaper snapshots`.
- **Secondary:** **Repair existing** remains visible but clearly maintenance-only (legacy rename / optimize / redundant JPG cleanup). Not required for post-update recovery.
- **Snapshot folders** list (`NewspaperSnapshotRootsSettings`):  
  - Keep status, **Open folder**, **Check again**.  
  - **Remove** per-row **Recover snapshot folder**. Path moves after a full recover are handled by picking the parent again (idempotent), or by Check + future reconnect only if we keep reconnect as a hidden offline fallback — default is remove the separate Recover CTA so settings are not duplicated.

### Toast / result payload

Surface counts, for example:

- editions imported / already known / skipped (non-matching dirs)
- clippings imported / already known / skipped (invalid layout)
- snapshot root: registered | reconnected | missing | marker mismatch (safe message)

Do not fail the whole recover if one side succeeds and the other partially fails; prefer a structured result with warnings over a single hard error when possible. Hard-fail only when the selected path is unusable (not a directory, path-safety rejection, etc.).

## Backend design

### Command shape

Prefer extending or replacing the Settings entrypoint so the UI calls one command, e.g. `recover_newspaper_library { path }` returning a typed counts struct (editions + clippings + root status + warnings). Keep `import_existing_newspaper_archive` as an internal helper or thin wrapper only if tests already depend on it; do not leave two user-facing recover commands.

Requires both `NewspaperState` (editions/pages DB) and `ClippingService` / writer paths (roots + clippings). Composition stays in the newspaper command layer; no new scheduler.

### Edition import (`archive_service`)

1. Skip `Newspaper snapshots`, `Youtubes`, and other non-edition trees by name/heuristics.
2. **Current layout:** for each immediate child matching `{anything} - {CODE}` where `CODE` is two ASCII uppercase letters present in `newspaper_editions` (`publication_date = ''`), and each date subfolder `YYYY-MM-DD` containing page images (`A01.webp`, etc.):
   - Group by `(code, date, date_dir)`.
   - Validate images; insert completed job + pages as today.
   - Idempotent on `(edition_code, publication_date, output_dir)` (existing `ON CONFLICT DO NOTHING`).
3. **Legacy layout:** keep `archive_identity` for `CODE_YYYYMMDD_….jpg|png|webp` files.
4. Do not invent source URLs; keep `archive://local` (or existing empty/local convention).

### Snapshot root

When `Newspaper snapshots` exists under the picked root (or the pick *is* that folder):

1. Read marker if present; register download destination / insert root using existing `register_download_destination` / marker rules where possible (parent destination = save-to root).
2. If DB already has this locator or matching `root_id`, treat as already known / reconnect locator if safe.
3. If marker mismatched with an existing offline root of the same id, update locator only when the selected tree verifies as the marked snapshot directory (reuse reconnect invariants: directory must be named `Newspaper snapshots`, marker must match `root_id`).
4. Never serialize absolute snapshot paths to React beyond existing display-path summaries.

### Clipping disk import

Walk under the snapshot root (skip `.linkvault`, skip top-level `assets` if it is the managed assets dir):

For each directory that contains `clipping-v1.webp`:

1. Parse **clipping id** from folder name: bare UUID, or trailing UUID after `Page … - `.
2. Parse **edition / date / page** from ancestors and optional `Page {page}` prefix; fall back to safe snapshots already used at create time (`edition_code_snapshot`, `edition_name_snapshot`, `publication_date_snapshot`, `page_number_snapshot`).
3. Compute `asset_relative_path` with existing `ClippingAssetLayout::snapshot_relative_path` (or equivalent) so media protocol resolution stays unchanged.
4. Read image dimensions / byte count / checksum as required by insert validation.
5. Read optional `note.md` (UTF-8); empty file → empty note; validate with existing `validate_note_markdown`.
6. Insert clipping as **ready** via `DatabaseWriter` / repository insert path; rebuild or update FTS the same way normal create/save does.
7. **Idempotent:** if clipping id already exists with same root (or same relative path), count as already known and skip overwrite of user note unless we explicitly decide “fill empty note only” — default: **do not overwrite** existing note or title.

### Repair

Unchanged behavior. Document in Settings hint that it is maintenance, not recovery. Optional follow-up: collapse under a disclosure; not required for this ship.

## Security / platform

- Path safety: reuse existing safe-directory / no-reparse checks used by clipping roots and archive import.
- All SQLite writes through `DatabaseWriter` for clipping mutations; edition import continues to follow current archive transaction patterns on the newspaper DB connection owned by that service (do not open a second writer).
- Blocking FS + image decode on `spawn_blocking`.
- No `unwrap`/`expect` on command paths.

## Testing

- Unit: edition scan recognizes `{zh} - BO/2026-08-09/A01.webp`; skips snapshots and Youtubes; legacy flat name still works.
- Unit: clipping walk imports UUID folder and `Page B02 - {uuid}` with `note.md`; second run `already_known`; empty note.md OK.
- Unit: picking parent registers snapshot root from real marker JSON.
- Regression: recover does not call repair’s delete-redundant-JPG path.
- Frontend: single recover button label; snapshot settings lack Recover CTA; toast uses structured counts.
- Use a fixture modeled on the real `Newpaper` tree (tiny webp + note), not production paths in CI.

## Success criteria

- Picking `…\Newpaper` after empty DB shows recovered editions in Newspaper library and clippings in Clippings with images and notes.
- Second recover does not duplicate rows; counts show already known.
- Settings show one recover entry point for library recovery; snapshot section is status/open/check only.
- Repair remains available and unchanged in behavior.

## Out of scope follow-ups

- Auto-recover when Newspaper save-to is first chosen (LinkedIn-style commit path).
- Importing clipping titles beyond folder/page heuristics.
- Merging Repair into Recover.
