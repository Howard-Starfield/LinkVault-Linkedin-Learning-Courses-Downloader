# YouTube helper supply chain

`youtube-helpers-lock.json` is the only source of truth for the Windows YouTube
helper inputs. It is intentionally marked `unpopulated` until a reviewed,
authoritative release choice supplies exact versions, HTTPS source URLs, asset
and source-archive sizes, SHA-256 values, archive-member identities, licenses,
and corresponding source records for yt-dlp, Deno, FFmpeg and FFprobe.

The empty lock is deliberate. `npm run verify:youtube-helpers` and the Rust
helper-integrity gate must fail closed while it remains unpopulated. The fetch
script is opt-in and never runs as part of a normal development or build
command. Do not copy a local Python launcher, a PATH installation, or an
unreviewed release into `apps/desktop/src-tauri/binaries/`.

The base `tauri.conf.json` deliberately omits the binaries so ordinary Rust
checks remain possible while this gate is closed. After the validated helper
inventory is staged, `npm run build:youtube-internal` first reruns the offline
verifier and then merges `src-tauri/tauri.youtube.conf.json`, whose
`bundle.externalBin` entries include the four target-triple executables. A
normal Tauri build is not a helper-enabled YouTube package.

After the exact metadata has been reviewed, set `status` to `ready`, populate
all four required components and their license/source records, then compute
`lockDigest` as the SHA-256 of the canonical JSON document with `lockDigest`
omitted. Run the verifier before any helper launch or package build.

## Research snapshot (non-enabling, 2026-08-21)

The following is upstream release evidence, not a populated lock. It does not
authorize downloading, packaging, or launching any helper.

### Compatible yt-dlp, EJS, and Deno candidate

The exact tagged `yt-dlp` source declares `yt-dlp-ejs==0.8.0` and pins
`deno==2.9.5` in its `pyproject.toml`. The official Windows standalone release
also bundles EJS, so a separate EJS executable is not required. The official
EJS release remains recorded for source/license review.

| Input | Pinned upstream evidence |
| --- | --- |
| yt-dlp | `2026.08.19`; official `yt-dlp.exe`, 17,840,399 bytes, SHA-256 `66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a`; [release](https://github.com/yt-dlp/yt-dlp/releases/tag/2026.08.19), [binary](https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe), [source archive](https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.tar.gz) (6,020,567 bytes, SHA-256 `072aad4f2a7604e92155f61a275a4752dc64046c8f6d90df3710525d94cd37c1`) |
| EJS | `0.8.0`; [tagged source](https://github.com/yt-dlp/ejs/releases/tag/0.8.0), source archive 96,571 bytes, SHA-256 `d5fa1639f63b5c4af8d932495f60689d5370f1a095782c944f7f62a303eb104e`; the tagged yt-dlp source requires this exact version |
| Deno | `2.9.5`; official x86_64 Windows archive, 42,691,248 bytes, SHA-256 `171efab55ac6b9881fd53ee4c20f8bf3bb1340ffc618483746909014db12216a`; [release](https://github.com/denoland/deno/releases/tag/v2.9.5), [archive](https://github.com/denoland/deno/releases/download/v2.9.5/deno-x86_64-pc-windows-msvc.zip) |

The standalone yt-dlp executable's EJS-bundling fact comes from the upstream
[EJS installation guidance](https://github.com/yt-dlp/yt-dlp/wiki/EJS). The
yt-dlp source uses the Unlicense; its bundled PyInstaller distribution carries
additional third-party notices. The EJS project uses the Unlicense with `meriyah`
(ISC) and `astring` (MIT) exceptions, and Deno's repository is MIT. These facts
still require exact committed notice files before a ready lock can be accepted.

### FFmpeg/FFprobe remains unselected

FFmpeg's [official download page](https://ffmpeg.org/download.html) points to
third-party Windows builders rather than publishing Windows executables. Two
currently observable candidates are:

- Gyan's `9.0.1` essentials ZIP, linked by the official FFmpeg page: 111,253,802
  bytes, SHA-256 `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9`,
  source commit `FFmpeg/FFmpeg@bf1b838f2a`; the builder describes its Windows
  builds as static GPLv3.
- BtbN's timestamped `autobuild-2026-08-21-13-40` `9.0.1-6-g9d4ca21220` LGPL
  static ZIP: 147,007,734 bytes, SHA-256
  `a2f743d8147830645a45c38656278952dffb7627f91b46ef1aea7089d0ddf542`.
  Its [release](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-08-21-13-40)
  is pinned by timestamp rather than the forbidden floating `latest` alias.

Neither candidate is ready for the lock. The archive checksum is not the
extracted `ffmpeg.exe` or `ffprobe.exe` checksum required by the verifier, and
the following must still be reviewed and recorded for the selected build:

1. exact archive member names, extracted sizes, SHA-256 values, PE identity, and
   the fact that both tools come from the same archive;
2. the corresponding FFmpeg source archive URL, size, and SHA-256 for the exact
   build commit;
3. the complete build configuration and dependency/license notices; and
4. the redistribution/source-offer treatment for the chosen GPL or LGPL
   variant.

Until those facts are verified from the builder and FFmpeg sources, the lock
must remain `unpopulated`, helper execution must remain disabled, and no
binary should be added to `apps/desktop/src-tauri/binaries/`.
