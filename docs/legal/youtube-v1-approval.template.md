# YouTube V1 product and legal approval

**Status:** Template only; not an approval or owner-risk acceptance

**Decision date:** YYYY-MM-DD

**Approval expiry / mandatory review date:** YYYY-MM-DD

**Product owner:**

**Legal/counsel reviewer, if required:**

**Reviewed specification:** `docs/specs/youtube-downloader-v1.md` at commit `<sha>`

**Reviewed specification blob SHA-256:** `<sha256-of-exact-prd-bytes>`

**Reviewed helper lock:** `docs/third-party/youtube-helpers-lock.json`

**Reviewed helper-lock digest:** `<rfc8785-derived-sha256>`

**Approved target triple and component versions:**

## Decision

Choose exactly one and explain the decision:

- [ ] Approved for implementation and distribution within the scope below.
- [ ] Approved for Y0-Y3 internal implementation/testing only; no public packaging, distribution or release.
- [ ] Rejected.

Decision rationale:

Only the first choice can authorize public distribution, and it still requires
the separate public-release review required by the PRD. The second authorizes
only the explicitly stated Y0-Y3 internal implementation/testing scope. It
never authorizes public packaging, distribution or release. Neither choice is
legal/counsel approval unless a qualified reviewer actually signs it; an owner
sign-off must not be represented as counsel approval or platform permission.

## Permitted scope

- Content/source types:
- User authorization requirements:
- Supported regions or distribution restrictions:
- Approved network-UAT content or approval process:

## Prohibited scope

- Browser-cookie or account authentication.
- Member-only, private, paid, age-gated or otherwise restricted content.
- DRM, access-control, rate-limit or platform-restriction bypass.
- Any additional restrictions:

## Approved user-facing copy

First-use acknowledgement:

Persistent YouTube-view guidance:

Error/help guidance:

## Evidence and follow-up

- Terms/policy sources reviewed with review dates:
- Third-party helper redistribution review:
- Required re-review triggers (including any spec/lock/component/copy change):
- Candidate-validation rule: exact spec bytes/commit, helper-lock digest,
  target and versions must match this decision.

## Sign-off

- Product owner, name/date:
- Legal/counsel reviewer, name/date, when applicable:

Validation fails when no choice or multiple choices are selected, a required
field/sign-off is blank, the decision is expired, or the candidate differs
from any pinned identity. For the internal owner-risk choice, the product
owner sign-off and explicit no-counsel/no-platform-permission statement are
required; counsel sign-off is not implied. Renaming or copying this template
never constitutes approval.
