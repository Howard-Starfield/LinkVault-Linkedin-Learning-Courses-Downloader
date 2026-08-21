# YouTube V1 internal owner-risk acceptance

**Status:** Accepted for Y0-Y3 internal implementation/testing only; public packaging, distribution and release are not approved

**Decision date:** 2026-08-20

**Approval expiry / mandatory review date:** 2026-09-19, or earlier on any material change or listed re-review trigger

**Product owner:** Howard Deng (user/project owner)

**Legal/counsel reviewer, if required:** Not requested or provided. This artifact is not legal or counsel approval.

**Reviewed specification:** `docs/specs/youtube-downloader-v1.md` at working-tree base `4735b416554ab43b06f08b7f532a150cdc238d51`, with the exact specification bytes pinned below

**Reviewed specification blob SHA-256:** `DF7580529A091718F4591989D8B64DA4A1E3569537B51D6F267E150CA5C7F681`

**Reviewed helper lock:** `docs/third-party/youtube-helpers-lock.json` — not yet created; Y0 must create and validate it before any helper execution

**Reviewed helper-lock digest:** `PENDING_Y0_HELPER_LOCK` — no helper launch is authorized while this remains pending

**Approved target triple and component versions:** `x86_64-pc-windows-msvc`; yt-dlp, Deno/EJS runtime, FFmpeg and FFprobe versions must be pinned by the Y0 helper lock before execution

## Decision

- [ ] Approved for implementation and public distribution within the scope below.
- [x] Approved for Y0-Y3 internal implementation/testing only; no public packaging, distribution or release.
- [ ] Rejected.

### Decision rationale

The project owner authorizes implementation and testing of the scoped YouTube
feature in isolated internal LinkVault builds. This owner-risk acceptance is
not legal advice, counsel approval, YouTube/platform permission, or a license
to copy third-party content. Public packaging, distribution and release are
reserved for a separate Y-PUBLIC-REVIEW decision.

## Permitted scope

- **Content/source types:** Public YouTube videos and explicit playlists that the user owns or is authorized to save; synthetic/local fixtures for automated tests.
- **User authorization requirements:** Before network UAT, the operator must confirm ownership or authorization to save each test source and must not treat LinkVault as granting permission.
- **Supported regions or distribution restrictions:** Internal developer/test builds on supported Windows machines only; no public distribution, hosted service, upload, or redistribution of downloaded content.
- **Approved network-UAT content or approval process:** Only public content owned by or expressly authorized to the user/project owner; record the source and authorization in the UAT evidence without committing media to the repository.
- **Authorized implementation phases:** Y0 helper/contract work, Y1 scan/transcript discovery, Y2 transcript-only execution and Y3 media execution, each subject to its documented automated, safety and native-UAT gates.

## Prohibited scope

- Browser-cookie, account or credential authentication.
- Member-only, private, paid, age-gated or otherwise restricted content.
- DRM, access-control, rate-limit or platform-restriction bypass, circumvention or evasion.
- Any public packaging, distribution, hosted deployment, public beta, or claim that LinkVault grants permission to copy content.
- Any helper launch before the exact Y0 lock, checksum, architecture, license and source validation passes.
- Any attempt to weaken these restrictions through arbitrary yt-dlp options, plugins, user configuration or alternate process paths.

## Approved user-facing copy

**First-use acknowledgement:**

> YouTube Downloader is for public videos and playlists you own or are authorized to save. Do not use LinkVault with private, member-only, paid, age-gated or otherwise restricted content. LinkVault does not grant permission, bypass access controls, or provide legal advice.

**Persistent YouTube-view guidance:**

> Use this internal feature only with public content you own or are authorized to save. Cookies, accounts, restricted-content access, DRM/access-control bypass and public distribution are not supported.

**Error/help guidance:**

> This item cannot be processed because it is unavailable or outside the authorized public-content scope. Do not add cookies or try to bypass access restrictions. Confirm that you own or are authorized to save the public source, then retry with a permitted URL.

## Evidence and follow-up

- **Terms/policy sources reviewed with review dates:** YouTube Terms of Service, <https://www.youtube.com/static?template=terms>, reviewed 2026-08-20 for risk boundaries and prohibited behavior. This review is not legal advice or a platform authorization.
- **Third-party helper redistribution review:** Pending Y0 helper-lock, source, checksum, license and clean-Windows verification; no public redistribution is authorized by this acceptance.
- **Required re-review triggers:** Any change to the PRD bytes, architecture, helper lock, helper/component versions, process or path boundaries, cookie/account or restricted-content scope, UI copy, distribution target, UAT source policy, or any incident involving bypass, unauthorized content or public exposure.
- **Candidate-validation rule:** Internal Y0-Y3 candidates must match the pinned specification bytes and this acceptance scope. The helper lock and integrity checks must pass before helper execution. Public packaging, distribution or release additionally requires a separate affirmative Y-PUBLIC-REVIEW decision with exact specification/helper-lock identity and packaged/native UAT evidence.

## Sign-off

- **Product owner, name/date:** Howard Deng (user/project owner), 2026-08-20
- **Legal/counsel reviewer, name/date, when applicable:** Not provided; no counsel approval claimed.

This artifact records the project owner's internal risk acceptance only. It
does not waive the prohibitions above and does not satisfy the separate
public-release review gate.
