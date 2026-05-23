# Desktop UAT Checklist

Run from `LinkVault/linkvault-tauri` on Windows PowerShell.

## Preflight

- [ ] `pnpm.cmd run verify:tauri-smoke` passes.
- [ ] `pnpm.cmd run verify:ui` passes.
- [ ] `pnpm.cmd run verify:visual` passes.
- [ ] `cargo test` passes from `src-tauri`.
- [ ] Debug executable exists at `src-tauri/target/debug/linkvault.exe`.

## Launch

```powershell
Start-Process .\src-tauri\target\debug\linkvault.exe
```

Expected:

- [ ] App opens to the LinkedIn Courses screen without a blank or crashed window.
- [ ] Sidebar shows LinkedIn Courses enabled and Generic Video disabled.
- [ ] Course setup, Activity, and Download Queue panels are visible at the default desktop size.
- [ ] Previous persisted safe settings and recent jobs load without exposing any plaintext token value.

## Native Folder Picker

- [ ] Click Browse beside Download folder.
- [ ] Native Windows folder picker opens.
- [ ] Choose a folder and confirm.

Expected:

- [ ] Download folder input updates to the selected folder.
- [ ] A success toast appears.
- [ ] No error toast appears.
- [ ] Reopening Browse starts from the current folder when Windows accepts the default path.

## Overlay Interactions

- [ ] Hover the settings icon.
- [ ] Click the help icon, then press Escape.
- [ ] Click the settings icon, then press Escape.

Expected:

- [ ] Settings tooltip appears on hover.
- [ ] Help popover opens and closes with Escape.
- [ ] Settings dialog opens, focuses its close button, closes with Escape, and returns focus to the settings icon.

## Guarded Download Flow

- [ ] Leave Course URLs empty and token empty.
- [ ] Confirm Start Download is disabled.
- [ ] Enter an invalid non-LinkedIn URL.
- [ ] Confirm the invalid URL toast appears and no queue row is persisted.
- [ ] Enter a LinkedIn Learning URL and no token.

Expected:

- [ ] URL validates, but Start Download remains disabled until a manual token or browser token path is available.
- [ ] Manual token input remains password-masked.

## Evidence To Record

- [ ] Windows version.
- [ ] App build type: debug or release.
- [ ] Whether Chrome/Edge/Firefox browser-token import was attempted.
- [ ] Any folder picker failure text, if seen.
- [ ] Any startup crash logs, if seen.
