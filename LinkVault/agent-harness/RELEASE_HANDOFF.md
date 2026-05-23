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
```

Expected:

- `verify:tauri-smoke` statically checks dialog plugin wiring, builds debug, launches `linkvault.exe`, and confirms it survives the startup smoke window.
- `verify:ui` covers browser-preview interaction flows and safe error rendering.
- `verify:visual` covers desktop, laptop, narrow, long-label, disabled-scope, guarded-start, and masked-token checks.
- `cargo test` covers backend parsing, auth, browser-cookie import, SQLite lifecycle, artifact download, cancellation, safe zip extraction, and live-client boundaries.

## Manual Gate Before Sharing Builds

- [ ] Complete `DESKTOP_UAT.md`.
- [ ] Run at least one browser-token import check on the target Windows account, if a valid LinkedIn session exists.
- [ ] Run at least one manual-token guarded flow, without recording or storing the token in logs.
- [ ] Confirm the selected download folder receives files only after an intentional Start Download.
- [ ] Confirm SQLite contains no plaintext `li_at`, cookie, or token setting keys after the flow.

## Packaging Notes

- Debug build output: `LinkVault/linkvault-tauri/src-tauri/target/debug/linkvault.exe`.
- Release packaging command: `pnpm.cmd tauri build` from `LinkVault/linkvault-tauri`.
- Keep release artifacts out of source commits unless explicitly requested.
- Before distributing, decide whether the first shared artifact should be a raw debug executable, release executable, MSI, or NSIS installer.

## Open Release Decisions

- [ ] App icon and Windows installer branding.
- [ ] Code signing strategy.
- [ ] Secret-store backend for any future saved credential support.
- [ ] Whether Generic Video stays disabled or is removed from release builds.
- [ ] Crash/log collection location and retention policy.
