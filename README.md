# LinkVault

LinkVault is a Tauri desktop app for archiving LinkedIn Learning courses into a local folder.

## Production App

The active production app lives in:

```text
apps/desktop
```

The old .NET/Avalonia project has been removed so production builds are unambiguous.

## Requirements

- Node.js with pnpm
- Rust toolchain
- Microsoft C++ Build Tools or Visual Studio Build Tools
- Microsoft WebView2 Runtime

## Install

```powershell
cd "C:\Users\howard\Downloads\Ai_script\Linkedin-Learning-Courses-Downloader-main"
pnpm.cmd --dir apps\desktop install
```

## Development

Run the Tauri desktop app:

```powershell
pnpm.cmd tauri dev
```

Run a browser-only frontend preview:

```powershell
pnpm.cmd dev
```

Then open:

```text
http://127.0.0.1:1420
```

## Production Build

From the repo root:

```powershell
pnpm.cmd tauri build
```

Equivalent direct command:

```powershell
pnpm.cmd --dir apps\desktop tauri build
```

Production outputs:

```text
apps\desktop\src-tauri\target\release\linkvault.exe
apps\desktop\src-tauri\target\release\bundle\nsis\LinkVault_0.1.0_x64-setup.exe
```

## Verification

Run the normal production verification set:

```powershell
pnpm.cmd verify:visual
pnpm.cmd verify:ui
pnpm.cmd verify:release
pnpm.cmd verify:installer
pnpm.cmd verify:release-manifest
```

`verify:release` runs the Tauri production build, verifies the release executable and NSIS installer exist, and smoke-launches the built app.

Backend tests:

```powershell
pnpm.cmd cargo:test
```

## Project Layout

```text
apps/
  desktop/
    src/              React frontend
    src-tauri/        Rust/Tauri backend and bundle config
    scripts/          UI, visual, release, and installer verification
docs/
  learning/           Personal learning notes and restored harness material
```

## Notes

Only download content you are allowed to access and archive. LinkVault does not bypass DRM, paid access controls, or site restrictions.
