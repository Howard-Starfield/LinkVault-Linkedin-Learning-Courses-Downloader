# Release Handoff

This is the release-prep checklist for the LinkVault Tauri migration.

## Current Automated Gate

Run from `LinkVault/linkvault-tauri`:

```powershell
pnpm.cmd run verify:tauri-smoke
pnpm.cmd run verify:ui
pnpm.cmd build
Set-Location .\src-tauri
cargo test
Set-Location ..
pnpm.cmd run verify:visual
pnpm.cmd tauri build --debug
pnpm.cmd run verify:release
pnpm.cmd run verify:installer
```

Expected:

- `verify:tauri-smoke` statically checks dialog plugin wiring, builds debug, launches `linkvault.exe`, and confirms it survives the startup smoke window.
- `verify:ui` covers browser-preview interaction flows and safe error rendering.
- `verify:visual` covers desktop, laptop, narrow, long-label, disabled-scope, guarded-start, and masked-token checks.
- `cargo test` covers backend parsing, auth, browser-cookie import, SQLite lifecycle, artifact download, cancellation, safe zip extraction, and live-client boundaries.
- `verify:release` asserts the checked-in Tauri bundle config, runs `pnpm.cmd tauri build`, requires `src-tauri/target/release/linkvault.exe`, requires an NSIS setup executable under `src-tauri/target/release/bundle/nsis`, launches the release executable through the startup smoke window, and terminates it cleanly.
- `verify:installer` checks the generated NSIS setup executable filename, version prefix, minimum size, and Windows PE header without running a system install.

## Manual Gate Before Sharing Builds

- [ ] Complete `DESKTOP_UAT.md`.
- [ ] Run at least one browser-token import check on the target Windows account, if a valid LinkedIn session exists.
- [ ] Run at least one manual-token guarded flow, without recording or storing the token in logs.
- [ ] Confirm the selected download folder receives files only after an intentional Start Download.
- [ ] Confirm SQLite contains no plaintext `li_at`, cookie, or token setting keys after the flow.

## Packaging Notes

- Debug build output: `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- First packaging target: release executable at `LinkVault/linkvault-tauri/src-tauri/target/release/linkvault.exe`.
- First installer target: NSIS setup executable under `LinkVault/linkvault-tauri/src-tauri/target/release/bundle/nsis/`.
- Checked-in installer config: `bundle.active = true`, `bundle.targets = ["nsis"]`, and `icons/icon.ico` in `src-tauri/tauri.conf.json`.
- Release verification command: `pnpm.cmd run verify:release` from `LinkVault/linkvault-tauri`.
- Installer artifact verification command: `pnpm.cmd run verify:installer` from `LinkVault/linkvault-tauri` after `verify:release`.
- Keep release artifacts out of source commits unless explicitly requested.
- MSI installer artifacts are deferred until installer branding and code-signing decisions are made.

## Open Release Decisions

- [ ] App icon and Windows installer branding.
- [ ] Code signing strategy.
- [ ] Secret-store backend for any future saved credential support.
- [ ] Whether Generic Video stays disabled or is removed from release builds.
- [ ] Crash/log collection location and retention policy.
