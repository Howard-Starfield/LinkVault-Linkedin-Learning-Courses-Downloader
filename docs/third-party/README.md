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
