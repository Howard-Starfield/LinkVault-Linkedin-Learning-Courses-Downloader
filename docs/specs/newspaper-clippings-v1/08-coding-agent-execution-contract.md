# Newspaper Clippings V1: coding-agent execution contract

**Status:** Approved

**Audience:** Any coding agent, automated contributor, or human implementer
working on an approved Newspaper Clippings V1 phase

**Authority:** This contract is mandatory. It supplements the repository’s
architecture, persistence, performance, release, and contribution conventions.
It does not authorize implementation while the master PRD says
`Implementation authorized: No`.

## 1. Mission

Implement exactly one approved Newspaper Clippings V1 phase at a time, prove
that phase against its requirements and exit gate, and stop for reviewer
approval before beginning later work.

The objective is not to produce the largest possible diff or to “finish the
feature” in one pass. The objective is to preserve user data and existing
LinkVault behavior while delivering independently reviewable, testable, and
reversible phase increments.

## 2. Mandatory preflight reading order

Before changing any code, read in this order:

1. `docs/architecture/README.md`
2. `docs/architecture/adr-001-unified-workflow-modular-monolith.md`
3. `docs/architecture/adr-002-newspaper-clippings-managed-assets.md`
4. `docs/specs/newspaper-clippings-v1/README.md`
5. `docs/specs/newspaper-clippings-v1/00-decision-register.md`
6. The detailed specification for the assigned phase.
7. Every prerequisite detailed specification named by the dependency graph.
8. Existing implementation files that own the affected behavior.
9. Existing verification scripts and tests for those files.

### AGENT-PREFLIGHT-001

Do not begin from a chat summary, issue title, mockup, or this execution
contract alone. The repository documents are the source of truth.

### AGENT-PREFLIGHT-002

Before editing, write a short implementation preflight in the PR description or
working notes containing:

```text
Assigned phase
Entry gate status
Authoritative documents read
Approved decisions relied on
Open decisions affecting this phase
Exact files expected to change
Exact files expected not to change
Tests/evidence to add
Exit-gate commands
Rollback boundary
```

### AGENT-PREFLIGHT-003

If the assigned phase entry gate is not satisfied, stop. Do not implement a
mock substitute, bypass, or “temporary” production behavior to work around a
missing prerequisite.

## 3. Authorization rules

Implementation is authorized only when all are true:

- ADR-002 status is `Accepted` or the repository’s equivalent approved status.
- The master PRD status permits implementation.
- The assigned phase status is Approved/Ready.
- Every prerequisite phase is merged to the target branch.
- The decision register has no Proposed decision that blocks the assigned phase.
- The implementation branch starts from the current target branch after those
  merges.

### AGENT-AUTH-001

A reviewer saying “continue” applies only to the assigned phase unless they
explicitly approve a different phase and update the control document.

### AGENT-AUTH-002

Do not infer approval from a branch name, TODO, partially implemented code,
existing dependency, or previous agent’s unstated intent.

## 4. One-phase rule

Each implementation PR owns one phase from the master PRD.

| Phase | Allowed primary scope | Explicitly prohibited in the same PR |
|---|---|---|
| 1 | Schema, repository, managed roots, asset lifecycle/recovery foundation, clipping media routes | Crop algorithm, reader Clip UI, editor dependency, production Clippings view |
| 2 | Source resolver, geometry, native crop, lossless encode, idempotent create command, crop baseline | Reader selection UI, editor/library, source navigation/delete UI |
| 3 | Reader selection state machine and hidden/test-only integration with Phase 2 command | Production Clippings view, editor dependency, deletion/reset UI, release bump |
| 4A | Isolated editor candidate evaluation, adapter proof, evidence, D-024 update | Production note view, unrelated UI refactor, persistence changes beyond test fixture needs |
| 4B | Production Clippings route/list/detail/source card/editor/autosave/conflict and reader enablement | Source-return highlight, final delete/reset implementation, OCR/AI/tags/export |
| 5 | Exact source navigation/back/highlight, delete, missing states, reset preservation integration | Performance threshold invention without Phase 6 evidence, release bump, V1 extensions |
| 6 | Full verification integration, measured thresholds, native UAT, security/license review, release readiness | New product features, schema redesign, unrelated refactors |

### AGENT-PHASE-001

When the phase exit gate passes, stop. Do not “use the remaining time” to begin
the next phase.

### AGENT-PHASE-002

A necessary prerequisite fix discovered during a phase must be handled by one of:

1. A small separately reviewed prerequisite PR.
2. An approved amendment to the current phase spec.
3. Stopping and reporting the blocker.

Do not hide prerequisite expansion inside the current diff.

## 5. Branch and PR convention

Recommended branch names:

```text
feat/newspaper-clippings-phase-1-persistence
feat/newspaper-clippings-phase-2-crop
feat/newspaper-clippings-phase-3-reader
spike/newspaper-clippings-phase-4a-editor
feat/newspaper-clippings-phase-4b-library-editor
feat/newspaper-clippings-phase-5-lifecycle
perf/newspaper-clippings-phase-6-release
```

Recommended titles:

```text
feat(newspaper): add clipping persistence and managed assets
feat(newspaper): add deterministic native clipping crop
feat(newspaper): add reader clipping selection workflow
test(editor): evaluate newspaper clipping note editors
feat(newspaper): add clippings library and note editor
feat(newspaper): add clipping source navigation and lifecycle controls
perf(newspaper): certify clipping performance and native release gates
```

### AGENT-PR-001

Do not include a release-version bump, tag, generated installer, or publication
step before Phase 6 is complete and the product owner requests release work.

### AGENT-PR-002

Prefer multiple focused commits when they aid review, but do not use commits to
hide that one PR spans multiple phases.

## 6. Required PR body

Every implementation PR uses this structure:

```markdown
## Assigned phase

Phase X — <name>

## Entry gate

- [ ] Prerequisite phases merged
- [ ] Blocking decisions approved
- [ ] Branch rebased on current target
- [ ] Relevant architecture/specifications read

## Requirements implemented

| Requirement/AC | Implementation | Test/evidence |
|---|---|---|
| `FR-...` | ... | ... |

## Files changed

### Expected
- `path`: reason and owning requirement

### Unexpected but necessary
- None, or link to approved scope amendment

## Deliberately out of scope

- ...

## Data and migration impact

- Schema version:
- Backup behavior:
- Existing-data compatibility:
- Rollback/roll-forward impact:

## Filesystem and asset impact

- Managed roots/files:
- Recovery behavior:
- Outside-root safety evidence:

## Security and privacy impact

- Path handling:
- Media/Markdown handling:
- Logging/redaction:
- Dependency/license changes:

## Performance impact

- Structural bounds:
- Release measurements:
- Known unmeasured areas:

## Automated verification

| Command | Commit | Result | Evidence |
|---|---|---|---|

## Manual/native verification

| Case | Environment | Result | Evidence |
|---|---|---|---|

## Failure-path evidence

- ...

## Rollback or forward-fix procedure

- ...

## Known limitations

- ...

## Stop confirmation

- [ ] This PR implements only the assigned phase.
- [ ] Later phases were not started.
- [ ] Existing gates were not weakened or skipped.
- [ ] The coding agent stopped after the exit gate.
```

### AGENT-PR-003

A requirement table containing only broad document links is insufficient. Map
the specific IDs implemented by the diff.

### AGENT-PR-004

Record failed commands and how they were resolved. Do not present only the final
green run if an intermediate failure revealed a design correction relevant to
review.

## 7. Change discipline

### Required behavior

- Preserve established provider boundaries.
- Keep Tauri commands thin.
- Reuse shared app database/storage services where the ADR assigns ownership.
- Add typed contracts instead of loosely shaped objects.
- Validate at trust boundaries.
- Add tests in the same PR as behavior.
- Keep deterministic helpers pure where specified.
- Prefer existing primitives and dependencies unless the phase explicitly
  authorizes a new dependency.
- Preserve existing command names and payloads unrelated to clipping.
- Preserve current Newspaper reader behavior and metrics.

### Prohibited behavior

- No new scheduler, workflow engine, generic notes platform, or background
  polling loop.
- No raw filesystem paths crossing IPC.
- No screenshot bytes as canonical clipping input.
- No database transaction across image, network, or external-process work.
- No direct clipping write connection outside `DatabaseWriter`.
- No source/job reset cascade into clipping deletion.
- No editor-specific JSON as durable note state.
- No raw executable HTML or MDX.
- No arbitrary local attachment support.
- No hard-coded “temporary” credentials, paths, test data, or user directories.
- No weakening/removal of architecture, persistence, performance, visual,
  security, or release checks.
- No `#[ignore]`, skipped Playwright case, broad lint disable, or catch-all
  error swallowing to obtain a green gate.
- No unrelated formatting/refactor churn that obscures the phase diff.
- No silent change to an Approved decision.

### AGENT-CHANGE-001

If an existing implementation contradicts the approved specification, implement
the approved behavior within the current phase scope and call out the change.
Do not treat accidental legacy behavior as stronger authority.

### AGENT-CHANGE-002

If an approved requirement is technically impossible under a verified platform
constraint, stop and submit evidence plus a proposed decision amendment. Do not
quietly implement an approximation.

## 8. Database and migration safeguards

Any phase that changes persisted schema or migration behavior must:

1. Inspect the current `CURRENT_SCHEMA_VERSION` after rebasing.
2. Advance from that actual version; never assume the version in the original
   draft PRD remains current.
3. Use the application initialization/migration boundary.
4. Preserve verified pre-migration backup.
5. Test fresh, previous, current, failure, and future-version cases.
6. Preserve representative data from every provider.
7. Run `foreign_key_check` and `quick_check`.
8. Keep runtime opens migration-free.
9. Document downgrade limitations and roll-forward strategy.

### AGENT-DB-001

Do not modify a user-facing database by manually running ad hoc SQL during
implementation or UAT. Use disposable copies and the application migration
path.

### AGENT-DB-002

When reset behavior changes, test the exact existing reset function and its
foreign-key-off defensive path. Do not create a second clipping-specific shadow
reset.

### AGENT-DB-003

A schema test that only checks table existence is incomplete. Verify
constraints, foreign-key actions, indexes, preservation, version, and recovery.

## 9. Managed filesystem safeguards

### AGENT-FS-001

All test files are under test-owned temporary roots. Tests place sentinel files
outside managed roots and assert they remain unchanged.

### AGENT-FS-002

Backend code derives canonical paths from validated IDs. It never accepts an
asset destination or relative path from React.

### AGENT-FS-003

Use create-new staging, validation, same-volume atomic promotion, recoverable
states, and safe cleanup as specified. Do not write canonical files in place.

### AGENT-FS-004

Treat symlinks and path escapes as security failures. Do not fall back to
ordinary delete/read behavior because a test environment makes them convenient.

### AGENT-FS-005

Do not recursively search arbitrary user folders to recover a missing asset.
Recovery is limited to exact managed paths and proven operation-owned staging,
trash, or quarantine entries.

## 10. Image and crop safeguards

### AGENT-CROP-001

The pure normalized-to-pixel algorithm in specification 03 is binding. Do not
change rounding based on visual preference or frontend estimates.

### AGENT-CROP-002

Prove losslessness with decoded pixel equality. File extension, encoder name,
or a quality setting is not proof.

### AGENT-CROP-003

Enforce source byte/dimension/pixel/output limits with checked arithmetic.

### AGENT-CROP-004

Handle or explicitly reject non-identity orientation. Never ignore it silently.

### AGENT-CROP-005

Record and enforce one concurrent full crop operation until Phase 6 measurement
and an approved decision change justify another bound.

### AGENT-CROP-006

The source page/media version must be rechecked before registration. A changed
page is not silently accepted.

## 11. Frontend and interaction safeguards

### AGENT-UI-001

Reader clipping is one interaction state machine. Do not add independent pointer
handlers that can run simultaneously with pan/click zoom.

### AGENT-UI-002

Preserve the reader’s at-most-three mounted page images. No hidden duplicate
images or full-page canvases.

### AGENT-UI-003

Use exact IDs for navigation, not virtual list/page indexes.

### AGENT-UI-004

Stale async responses carry a request generation or document identity and
cannot overwrite current state.

### AGENT-UI-005

Dirty note state is flushed or explicitly discarded before navigation. Do not
navigate first and hope autosave finishes after unmount.

### AGENT-UI-006

Do not expose Phase 3 capture in normal production navigation until Phase 4B
provides the complete review/note destination.

### AGENT-UI-007

Every state must have loading, empty, failure, retry, keyboard, focus, and
accessibility behavior; success-only UI is incomplete.

## 12. Editor dependency safeguards

Only Phase 4A may select the editor dependency.

### AGENT-EDITOR-001

Evaluate at least two current candidates using the same fixture and native IME
matrix. Do not select solely from README claims or model familiarity.

### AGENT-EDITOR-002

Record exact versions, primary-source compatibility evidence, bundle delta,
license, security configuration, and rejected reasons.

### AGENT-EDITOR-003

Remove rejected candidate packages and experimental code before Phase 4A exits.

### AGENT-EDITOR-004

Update D-024 and obtain approval before Phase 4B. Installing a package in Phase
4B without this approval violates the entry gate.

### AGENT-EDITOR-005

Keep the package behind `ClippingNoteEditor`. No production caller imports it
directly.

### AGENT-EDITOR-006

Do not claim Chinese IME support from browser automation alone. Record installed
Windows evidence.

## 13. Testing rules

### AGENT-TEST-001

Every requirement that can fail has at least one positive and one relevant
negative/failure-path test.

### AGENT-TEST-002

Use deterministic generated fixtures where possible. Do not depend on live
World Journal, network availability, clock timing without fake/control, or the
user’s existing downloads.

### AGENT-TEST-003

Concurrency tests must prove bounds dynamically and structurally; sleeping and
assuming overlap is insufficient when a barrier/latch can make it deterministic.

### AGENT-TEST-004

Crash recovery tests instantiate durable intermediate states directly in a
temporary copy, restart the service/recovery, and verify repeated idempotence.
They do not merely unit-test a state enum.

### AGENT-TEST-005

Browser tests assert instrumentation for mounted images/rows, request counts,
and state transitions. Visual screenshots alone do not prove bounded behavior.

### AGENT-TEST-006

Never modify expected fixtures to match an unintended implementation result
without reviewing the governing requirement.

### AGENT-TEST-007

Any test skipped due to environment must be listed as an unmet exit gate unless
the specification explicitly classifies it as later native UAT.

## 14. Performance rules

### AGENT-PERF-001

Preserve binding structural budgets from specification 07 even before measured
latency thresholds are ratified.

### AGENT-PERF-002

Use release builds for performance claims. Development mode may identify
problems but cannot certify responsiveness.

### AGENT-PERF-003

Record hardware, OS, commit, fixture, sample method, durations, memory evidence,
and raw artifact location. Do not report one unexplained average.

### AGENT-PERF-004

Do not introduce an arbitrary latency threshold just to make a gate appear
objective. Collect the specified baseline, compare existing behavior, and have
Phase 6 ratify a threshold.

### AGENT-PERF-005

A faster implementation that violates pixel correctness, recovery, privacy, or
memory bounds is not accepted.

## 15. Security and privacy rules

### AGENT-SEC-001

Review every IPC field as untrusted. Frontend validation improves UX but does
not replace Rust validation.

### AGENT-SEC-002

Return stable safe error codes/messages. Do not expose raw IO/SQL/decoder errors
when they contain paths or implementation details.

### AGENT-SEC-003

Diagnostics contain safe operation/provider/ID/timing/classification fields,
not note text, title text, image bytes, source URLs with credentials, cookies,
tokens, or absolute paths.

### AGENT-SEC-004

Search uses bound SQL parameters and explicit wildcard escaping.

### AGENT-SEC-005

Markdown rendering/editor configuration disables executable MDX/raw HTML and
unsafe URL schemes. Test the configured production path, not only a helper.

### AGENT-SEC-006

New dependencies require primary-source license/security/compatibility review
and any needed third-party notice update in the same approved phase.

## 16. Ambiguity and conflict procedure

Stop and request a specification decision when:

- Two authoritative documents conflict after applying the precedence order.
- The required command/API shape cannot fit existing Tauri serialization without
  a material contract change.
- Existing source ownership differs from ADR-002.
- A platform/library limitation affects approved product behavior.
- A test reveals undefined deletion/recovery/data-loss semantics.
- A new dependency is required outside an authorized phase.
- A later-phase feature appears necessary to expose an earlier phase safely.
- A proposed optimization changes a binding correctness/security invariant.

Use this issue format:

```markdown
## Specification ambiguity/blocker

Phase:
Requirement(s):
Observed repository/platform behavior:
Why current approved behavior cannot be implemented safely:
Options considered:
Recommended decision:
Migration/data impact:
Security/privacy impact:
Test/gate impact:
Files/documents requiring amendment:
```

### AGENT-AMBIGUITY-001

Do not resolve product ambiguity by choosing the option that is easiest to code.

## 17. Stop conditions

The agent must immediately stop implementation when:

- Entry gate is not met.
- Current branch contains unreviewed changes outside phase scope.
- A required migration backup/integrity test fails.
- A path-security test fails.
- A clipping or note is lost in a failure/reset test.
- The reader mounted-image bound regresses.
- Exact pixel correctness fails.
- Chinese IME fails in Phase 4A/4B native testing.
- A required existing gate fails and the failure is not demonstrably unrelated.
- Completing the work requires beginning a later phase.
- A blocking decision remains Proposed.

Stopping means report evidence and await a reviewer; it does not mean deleting
failing tests, weakening validation, or replacing the requirement.

## 18. Phase completion handoff

At the end of a phase, produce:

1. PR requirement/evidence table.
2. Complete command results.
3. Native/manual evidence required by that phase.
4. Performance artifact required by that phase.
5. Migration and recovery evidence when applicable.
6. Security/privacy/dependency notes.
7. Known limitations.
8. Rollback/forward-fix instructions.
9. Exact follow-up phase now unblocked.
10. Explicit statement: `No work from later phases is included.`

Do not automatically open or begin the next implementation branch unless the
reviewer explicitly requests it after merge.

## 19. Suggested coding-agent prompt

A reviewer may start an implementation agent with this template:

```text
Implement Newspaper Clippings V1 Phase <X> only in
Howard-Starfield/LinkVault-Linkedin-Learning-Courses-Downloader.

Before editing, read the repository architecture documents and the complete
Newspaper Clippings V1 master/decision/current-phase specifications. Verify the
phase entry gate and report your planned files, requirements, tests, and exit
commands.

Follow docs/specs/newspaper-clippings-v1/08-coding-agent-execution-contract.md.
Do not implement later phases, add unapproved dependencies, weaken tests, expose
raw paths, or change approved decisions. Add failure-path coverage and run the
entire phase exit gate. Stop after the phase gate and prepare a PR with the
required evidence table.
```

The prompt does not replace the documents and should not paste a stale copy of
requirements into the agent context as a new source of truth.

## 20. Definition of done for an implementation PR

An implementation PR is done only when:

- Its assigned phase and entry gate are explicit.
- Every implemented behavior maps to requirement/AC IDs.
- Required positive, negative, failure, and recovery tests exist.
- All phase exit commands pass on the head commit.
- Required release/native evidence is committed or linked.
- No later-phase code or V1 extension is included.
- No existing gate is weakened or skipped.
- Data migration, reset, recovery, privacy, security, and rollback impacts are
  documented accurately.
- Review comments are resolved through code/spec changes, not hidden workarounds.
- The agent has stopped and awaits approval.

Passing compilation alone, producing a convincing demo, or satisfying the happy
path is not done.
