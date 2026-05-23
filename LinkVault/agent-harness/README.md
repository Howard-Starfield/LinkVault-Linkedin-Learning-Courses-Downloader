# LinkVault Local Migration Harness

This harness lives inside `LinkVault/` by request. Use it as the local control plane for the Tauri 2 + Rust + React migration.

## Canonical References

- Visual/design system reference: `../design.md`
- Screenshot reference: `../reference.png`
- Attached planning image: Image #1 in the conversation, visually equivalent to `../reference.png`

The implementation must reference both `design.md` and `reference.png` during UI work. Do not treat either as decorative context; they are acceptance inputs.

## Product Scope

Build LinkVault as a LinkedIn Learning course downloader only.

Preserve:

- 1080p best-available default download behavior.
- 1080 -> 720 -> 540 -> 360 fallback.
- Exercise file download.
- Exercise zip auto-unzip.
- Safe zip extraction.
- Duplicate exercise wrapper cleanup.
- Delete zip only after successful extraction.
- Transcript/subtitle download.
- Browser token import from Chrome, Edge, and Firefox.
- Manual token paste.
- SQLite local cache for settings, job state, course cache, and artifacts.

Do not port Generic Video, public playlist download, or LinkedIn Scraper for the MVP. The reference image may show those nav rows, but they are visual-context only unless explicitly re-scoped later.

## How Future Agents Should Work

1. Read `STATUS.md`, `TODO.md`, `REFERENCE_CONTRACT.md`, and `EDGE_CASE_MATRIX.md`.
2. Pick one small task.
3. Keep edits either inside `LinkVault/` or inside the final scaffold path named in `STATUS.md`.
4. If implementation leaves `LinkVault/`, update this harness first with the reason and new owned path.
5. Add deterministic tests before live LinkedIn checks.
6. Update `STATUS.md` and `TODO.md` after the slice.

