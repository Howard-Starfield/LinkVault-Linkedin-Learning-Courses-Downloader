# YouTube helper notices

The internal YouTube candidate acquires the exact helper bytes recorded in
`youtube-helpers-lock.json`. Helper executables are build inputs and are not
committed to Git.

- yt-dlp `2026.08.19` is released under the Unlicense. Its official standalone
  executable embeds Python, PyInstaller bootloader material, yt-dlp EJS `0.8.0`,
  and other runtime dependencies. The exact upstream executable and tagged
  source archive are recorded in the lock.
- Deno `2.9.5` is released under the MIT License. The lock records the official
  Windows x86_64 archive and the exact tagged source archive.
- FFmpeg and FFprobe come from BtbN build
  `n9.0.1-6-g9d4ca21220-20260820`, release
  `autobuild-2026-08-20-13-45`, using the builder's static LGPL dependency set
  with `--enable-version3`. The archive's `LICENSE.txt` is byte-identical to
  FFmpeg commit `9d4ca21220bfd3f06fc8bfc90ddf0f6d0a484611`'s
  `COPYING.LGPLv3`; the lock records that exact corresponding-source archive,
  distribution archive, members, and hashes.

The committed license copies are:

- `youtube/yt-dlp-2026.08.19-LICENSE.txt`
- `youtube/deno-2.9.5-LICENSE.txt`
- `youtube/ffmpeg-9d4ca21220-LGPLv3.txt`

This record supports the owner-authorized internal implementation and testing
candidate only. Public packaging, redistribution, or release remains blocked
pending a separate review of all executable-bundle dependency notices and the
chosen FFmpeg build's corresponding-source and relinking obligations. This
notice is not legal advice and does not authorize restricted-content access.
