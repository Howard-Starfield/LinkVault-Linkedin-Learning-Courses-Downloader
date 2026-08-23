# YouTube helper supply chain

`youtube-helpers-lock.json` is the only source of truth for the Windows YouTube
helper set. The reviewed internal-test lock is `ready` with canonical digest
`f2eb38349e71bd05b8da27807bc82e5eecb204cbf8b335952276bc1786527b7c`.
It pins the distribution asset, extracted executable where applicable,
corresponding source archive, license bytes, notice bytes, compatibility facts,
and target triple for each component.

The fetch script is opt-in and never runs during a normal development or build
command. Do not copy a PATH installation, Python launcher, floating release, or
unreviewed executable into `apps/desktop/src-tauri/binaries/`.

## Reviewed internal helper set (2026-08-23)

| Component | Reviewed identity |
| --- | --- |
| yt-dlp | `2026.08.19`, official Windows standalone executable; bundled EJS `0.8.0` |
| Deno | `2.9.5`, official `x86_64-pc-windows-msvc` archive and extracted executable |
| FFmpeg | BtbN LGPL static build `n9.0.1-6-g9d4ca21220-20260820`, commit `9d4ca21220bfd3f06fc8bfc90ddf0f6d0a484611` |
| FFprobe | The FFprobe executable from the same pinned BtbN archive/build as FFmpeg |

The authoritative byte sizes, SHA-256 digests, URLs, archive members, source
records, license records, notices, and compatibility fields are in the lock.
The offline verifier checks the committed license and notice bytes as well as
every fetched executable before the Rust process boundary can resolve it.

FFmpeg does not publish official Windows binaries. Its official download page
links third-party builders; this branch pins one timestamped BtbN LGPL static
build instead of a floating `latest` URL. This selection is approved only for
the internal candidate described in `docs/legal/youtube-v1-approval.md`.

## Lock and packaging rules

Every component must declare an exact safe `filename`/`path`, distribution and
source archive formats, compatibility object, and complete `sourceRecord`,
`licenseRecord`, and `noticeRecord`. Archived assets also declare the exact
member, extracted size, and extracted SHA-256. FFmpeg and FFprobe must bind the
same non-null build ID; yt-dlp must bind its compatible EJS version. Floating
versions and URLs are rejected.

Only a `ready` lock with a matching canonical digest may be fetched, resolved,
or packaged. `npm run fetch:youtube-helpers` follows only bounded,
credential-free HTTPS redirects and atomically promotes files after size and
SHA-256 verification. `npm run verify:youtube-helpers` rechecks the complete
local inventory offline.

The base `tauri.conf.json` deliberately omits the helpers. The explicitly
internal `npm run dev:youtube-internal` and `npm run build:youtube-internal`
commands first rerun verification and then merge
`src-tauri/tauri.youtube.conf.json`, whose `bundle.externalBin` entries name the
four target-triple executables. A normal Tauri dev/build command is not a
helper-enabled YouTube candidate.

Public packaging, redistribution, hosted use, and public release remain
blocked pending a separate owner/legal review. Generated helper binaries,
installers, UAT downloads, logs, and evidence directories are not committed.
