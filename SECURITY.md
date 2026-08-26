# Security Policy

## Supported versions

LinkedVault is a Windows desktop application. Security fixes target:

| Version | Supported |
| --- | --- |
| Latest GitHub release of the desktop app | Yes |
| Current `main` branch | Yes (development builds) |
| Older release tags | Best-effort only; upgrade to the latest release when possible |

The current packaged version is tracked in `apps/desktop/package.json` and
`apps/desktop/src-tauri/tauri.conf.json` (see also [CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

**Do not open a public GitHub issue for security reports.**

Prefer one of these private channels:

1. **GitHub Security Advisories** (private vulnerability reporting) on this
   repository, if enabled for the project.
2. Contact the **repository maintainers** through GitHub using a private
   channel (for example a maintainer’s GitHub security contact), without posting
   exploit details publicly.

Please include:

- A short description of the issue and its impact
- Steps to reproduce or a minimal proof of concept when safe to share
- Affected version or commit, OS build, and whether you used an installer or a
  local `npm run dev` build

We will acknowledge reports as soon as practical and coordinate disclosure after
a fix is available or a mitigation is agreed.

## Scope (high level)

In scope for this project includes, for example:

- Local desktop app integrity and update trust assumptions
- Credential handling (including Windows DPAPI-protected session material)
- Path safety for downloads, archives, clipping assets, and app data roots
- SQLite and filesystem durability boundaries that could lead to data loss or
  unintended local file access

## Out of scope

- Requests to bypass a provider’s terms of service, DRM, paid access controls,
  robots.txt, or rate limits
- Social engineering of LinkedIn, Coursera, World Journal, or other third-party
  accounts
- Issues that only affect unsupported or heavily modified local forks

## Responsible use

LinkedVault is intended for local archives of content you are allowed to access.
See the privacy and responsible-use section in [README.md](README.md).
