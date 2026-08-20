# LinkVault

![How to find your LinkedIn li_at cookie](apps/desktop/src/assets/guide.png)

LinkVault is a local-first desktop application for organizing and archiving learning materials and publications that users are authorized to access and save. It can work with supported sources such as LinkedIn Learning, Coursera, and World Journal to create organized local archives for offline study and personal knowledge management.

LinkVault is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by LinkedIn, Coursera, World Journal, or their respective owners.

## What You Can Do

- Download course videos into a folder you choose.
- Save subtitles, exercise files, quiz notes, and a readable `Study.md` guide when available.
- Queue multiple course links and track progress in one place.
- Retry failed downloads without rebuilding the whole list.
- Save your cookie once, then reuse it for future launches.
- Keep data local to your machine.
- Download daily, weekly, and discovered special World Journal editions.
- Schedule newspaper batches with a configurable delay between editions.
- Optimize newspaper pages as high-clarity WebP while safely retaining originals when needed.
- Browse shallow front-page previews and read downloaded editions offline.
- Register an existing Newspaper Extractor archive without moving its files.

## How To Use

1. Install LinkVault with the Windows installer.
2. Open LinkVault.
3. Paste one or more LinkedIn Learning course URLs.
4. Choose the download folder and quality.
5. Paste your LinkedIn `li_at` cookie once, or use a supported browser session.
6. Click **Start Download**.

For newspapers, open **World Journal → Download editions**, select editions and dates, then choose **Download now** or **Schedule downloads**. Completed and partial editions appear under **Newspaper library**.

Your downloads are saved into the folder you picked. Your saved session is protected with Windows encryption and stored locally.

## Requirements

- Node.js with npm
- Rust toolchain
- Microsoft C++ Build Tools or Visual Studio Build Tools
- Microsoft WebView2 Runtime

## Run Locally

Use this when you want to work on the app from source.

```powershell
git clone https://github.com/Howard-Starfield/LinkVault-Linkedin-Learning-Courses-Downloader.git LinkVault
cd LinkVault
npm --prefix apps\desktop install
npm run dev
```

If PowerShell blocks `npm.ps1`, either run this once:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

or use the `.cmd` shim:

```powershell
npm.cmd run dev
```

The desktop app opens from Tauri. The Vite frontend runs at:

```text
http://127.0.0.1:1420
```

Stop the development app with `Ctrl+C` in the terminal that launched it.

### Developer test: Newspaper Clippings

Run this test in the native Tauri window, not the frontend-only browser preview.
Desktop commands, managed snapshot files, and native input behavior require the
Tauri runtime.

Before testing, make sure **World Journal → Newspaper library** contains at
least one downloaded edition with a readable completed page.

1. Open an edition, choose **Clip**, and drag over part of one newspaper page.
   Confirm that the selection follows the pointer, then choose **Save clipping**.
   The reader must remain open and offer **Open note** after the save succeeds.
2. Open **World Journal → Clippings**. At the default desktop width, confirm
   that four thumbnails fit on each row. Resize the window and confirm that the
   responsive column count changes without stretching the crop. Thumbnails
   should replace their placeholders without flashing back to an empty image.
3. Hover a thumbnail. Only its image should enlarge slightly; the card must not
   jump upward. Select the thumbnail to open its separate note page.
4. Confirm the detail page has a compact **Back** action with the editable note
   title beside it, no search box, and no boxed editor card. The saved crop
   should be the large read-only header above its provenance and note body.
   Save state plus Undo/Redo belong in the bottom-right note footer.
5. At the start of a paragraph or after a space, type `/`. Confirm the popup is
   visible above the note, then filter it and exercise **Text**,
   **To-do list**, **Heading 1–4**, **Bullet list**, **Numbered list**,
   **Quote**, and **Divider** with both pointer selection and the arrow keys
   plus `Enter`. Confirm `/h` selects **Heading 1**, `/todo` selects
   **To-do list**, `/hr` selects **Divider**, and a close typo such as
   `/heding` still ranks **Heading 1** first without executing it automatically.
6. Drag across note text with the left mouse button. No formatting toolbar
   should appear during the drag. After release, it should appear above the
   selection, aligned with the first selected word. Check **Bold**, **Italic**,
   **Strikethrough**, and **Link**.
7. Paste plain text, then try pasting an image or file. Text must remain usable;
   image/file paste must be rejected without replacing the saved clipping.
8. Edit the title and note, wait for **Saved**, choose **Back**, and reopen the
   clipping. Confirm that the title, Markdown formatting, and note content were
   persisted. Optionally repeat typing with a Chinese IME to verify native
   composition does not duplicate or lose committed text.
9. Open that clipping's snapshot directory. Confirm `note.md` sits beside
   `clipping-v1.webp` and contains the saved Markdown. SQLite remains canonical:
   editing `note.md` externally is not imported and may be overwritten by the
   next save or startup repair.
10. Use the Clippings search box. Search separately for words found in the title,
   note, edition, date, and page. Confirm the matching field tags are correct,
   lower-confidence results are separated as **Possible matches**, and more
   results load while scrolling. The search box must remain exclusive to the
   gallery/search surface.
11. In **Settings → Snapshot locations**, confirm the derived root is connected.
    On disk, the clipping belongs under the same newspaper download destination
    at `Newspaper snapshots/<edition>/`; Settings must not offer an arbitrary
    global snapshot-folder override.
12. Add a unique title/body suffix and click the window **X** before the footer
    reaches **Saved**. The window should hide instead of exiting. Choose
    **Show LinkVault** from the tray, reopen the clipping, and confirm the exact
    suffix is canonical or appears in the explicit recovery state. There must
    still be only one main window.
13. Type continuously, click **X** while the footer says **Saving…**, then show
    LinkVault from the tray. Confirm the newest text—not an earlier keystroke—is
    present. Repeat the hide/show cycle once to catch duplicate close handlers.
14. While LinkVault is running, launch it again from the Start menu or installed
    executable. Confirm the existing window is shown and focused, Task Manager
    still reports one `linkvault.exe`, and the current clipping note remains
    visible after its durability flush/refresh.
15. Add another unique suffix and immediately choose **Quit** from the tray.
    Relaunch LinkVault and confirm the exact latest text is saved or explicitly
    offered for recovery. Tray Quit must exit; window X must only hide.
16. Do not use Task Manager against a real user database to test crash recovery.
    Forced-termination and injected save/checkpoint failures belong to the
    isolated automated harness or a disposable test profile.

Focused automated checks for this workflow:

```powershell
npm --prefix apps\desktop run verify:clipping-note-editor-markdown
npm --prefix apps\desktop run verify:clipping-note-editor
npm --prefix apps\desktop run verify:clipping-note-autosave
npm --prefix apps\desktop run verify:clipping-note-lifecycle
npm --prefix apps\desktop run verify:clipping-note-durability-structure
npm --prefix apps\desktop run verify:clipping-note-durability-browser
npm --prefix apps\desktop run verify:newspaper-clipping-library
```

## Architecture

Backend ownership, the unified workflow decision, and the provider migration
roadmap are documented in [`docs/architecture`](docs/architecture/README.md).
Read that contract before adding a provider, queue, scheduler, background
worker, or persisted job state.

## Build The App

Use this when you want a production executable or Windows installer.

```powershell
npm run tauri -- build
```

Production outputs:

```text
apps\desktop\src-tauri\target\release\linkvault.exe
apps\desktop\src-tauri\target\release\bundle\nsis\LinkVault_<version>_x64-setup.exe
```

## Publish An Update

The release workflow runs when you push a version tag like `vX.Y.Z`. It builds the Windows installer, creates a GitHub release, and uploads `latest.json` for the in-app updater.
Tags with a prerelease suffix, such as `vX.Y.Z-rc.1`, are published as GitHub prereleases and do not replace the stable `latest` release.

1. Bump the version in `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, and `apps/desktop/src-tauri/tauri.conf.json`.
2. Run:

```powershell
npm run build
npm run cargo:test
```

3. Commit and push the version bump.
4. Create and push the tag:

```powershell
git tag vX.Y.Z
git push origin main
git push origin vX.Y.Z
```

Users on older signed builds can use the in-app update button after the GitHub release finishes.

## Frontend-Only Preview

This is useful for UI work, but desktop-only features need Tauri.

```powershell
npm run web:dev
# open:
http://127.0.0.1:1420
```

## Verify A Release

Useful checks before sharing a build:

```powershell
npm run cargo:test
npm run verify:visual
npm run verify:ui
npm run verify:release
npm run verify:installer
npm run verify:release-manifest
```

## Responsible Use / Third-Party Services

LinkVault is intended for personal archiving, offline study, and knowledge management of material that you are legally and contractually permitted to access, download, or export.

- You are responsible for complying with applicable laws, copyright rules, licenses, and the terms of service of any third-party provider you use with LinkVault.
- Access to a service or possession of valid account credentials does not automatically grant permission to copy, redistribute, or retain its content.
- LinkVault does not grant any license or other rights to third-party content, services, trademarks, or APIs.
- LinkVault is not intended to bypass DRM, authentication, paywalls, access controls, or other technical restrictions.
- Do not use LinkVault to redistribute copyrighted material or content you do not have permission to save.

Third-party services can change their rules, APIs, authentication methods, and technical restrictions at any time. Users should confirm that their intended use is permitted by the relevant provider.

## License

LinkVault is open source and licensed under the [MIT License](LICENSE).

The MIT License applies to LinkVault's original source code and documentation. It does not grant rights to third-party content, services, trademarks, APIs, or other materials accessed through the software.

Third-party dependencies remain governed by their respective license terms. See `THIRD_PARTY_NOTICES.md` for details.

Copyright (c) 2026 Howard Deng.
