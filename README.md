# LinkVault

LinkVault helps you save LinkedIn Learning courses you already have access to into a local folder, so your learning materials are easier to organize, revisit, and study offline.

## What You Can Do

- Download course videos into a folder you choose.
- Save subtitles, exercise files, and quiz notes when available.
- Queue multiple course links and track progress in one place.
- Retry failed downloads without rebuilding the whole list.
- Save your cookie once, then reuse it for future launches.
- Keep data local to your machine.

## How To Use

1. Install LinkVault with the Windows installer.
2. Open LinkVault.
3. Paste one or more LinkedIn Learning course URLs.
4. Choose the download folder and quality.
5. Paste your LinkedIn `li_at` cookie once, or use a supported browser session.
6. Click **Start Download**.

Your downloads are saved into the folder you picked. Your saved session is protected with Windows encryption and stored locally.

## Requirements

- Node.js with pnpm
- Rust toolchain
- Microsoft C++ Build Tools or Visual Studio Build Tools
- Microsoft WebView2 Runtime

## Build The App

```powershell
cd "C:\Users\howard\Downloads\Ai_script\Linkedin-Learning-Courses-Downloader-main"
pnpm.cmd --dir apps\desktop install
pnpm.cmd tauri build
```

Production outputs:

```text
apps\desktop\src-tauri\target\release\linkvault.exe
apps\desktop\src-tauri\target\release\bundle\nsis\LinkVault_0.1.0_x64-setup.exe
```

## Run For Development

```powershell
pnpm.cmd tauri dev
```

Browser-only preview:

```powershell
pnpm.cmd dev
```

Then open:

```text
http://127.0.0.1:1420
```

## Verify A Release

Useful checks before sharing a build:

```powershell
pnpm.cmd cargo:test
pnpm.cmd verify:visual
pnpm.cmd verify:ui
pnpm.cmd verify:release
pnpm.cmd verify:installer
pnpm.cmd verify:release-manifest
```

## Responsible Use

Only download content you are allowed to access and archive. LinkVault does not bypass DRM, paid access controls, or site restrictions. All right reserved!
