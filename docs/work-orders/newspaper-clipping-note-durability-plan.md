# Newspaper clipping note durability implementation plan

**Status:** Approved Phase 4C implementation work order. Product defaults are
approved; implementation still follows the branch/entry gates in this document
and does not authorize a push, migration of user data, or release by itself.

**Date:** 2026-08-10

**Audited Phase 4B code base:** `12a65a1` (`feat(newspaper): finish Phase 4B
clipping notes`). The final Phase 4B closeout passed build, architecture,
persistence (44/44), UI, visual geometry, editor (17/17), clipping library
browser, Newspaper static/browser performance, full Rust (540 passed/4
documented ignored), dependency audit, and composite release verification.

**Scope:** Newspaper Clippings note autosave, recovery checkpoints, editor
unmount, main-window close, tray Quit, application exit, updater exit, and
accepted database-write shutdown ordering.

**Target outcome:** A clipping note that is visible to the user is never
silently discarded by ordinary navigation, rapid editor close, Windows close,
tray Quit, or application shutdown. A crash or forced termination recovers the
latest bounded native checkpoint without allowing draft content to corrupt the
canonical note, revision contract, or clipping search index.

## 0. Authority and entry gate

This plan intentionally exceeds the original V1 durability boundary. Approved
D-034 now supersedes D-026 and `FR-AUTOSAVE-008` only where they accepted
forced-termination loss inside the 800 ms debounce and prohibited a second
local draft journal. D-034 approves schema version 6 for this bounded recovery
table while retaining the canonical autosave and revision contracts.

Before Slice B or any production recovery implementation:

1. Confirm D-034 and the aligned specifications remain approved after rebase.
2. Preserve the approved close-X behavior and recovery-only byte envelope in
   section 3. Both were approved by the product owner on 2026-08-10.
3. Use the approved Phase 4C implementation/PR boundary: a separately reviewed
   durability follow-up, not a hidden schema expansion inside Phase 4B UI work.
4. Re-run the live owner/line-count audit because the current worktree is still
   uncommitted and may change before implementation begins.
5. Confirm Phase 4B's editor/autosave owners are available on the Phase 4C base
   and the working tree contains no unrelated unreviewed changes.

The approved close-X behavior may be used when the lifecycle slice is
authorized. No user database may be migrated merely by the existence of this
work order; production work still requires the Phase 4C entry gate and a
disposable migration fixture before native UAT.

## 1. Planning evidence and current owner audit

This work order was written after inspecting the live Phase 4B worktree and the
existing Infield note lifecycle. It is not based on an assumed canonical React
component.

Measured production owners, refreshed after the Phase 4B closeout audit:

| Current owner | Lines | Current responsibility | Durability constraint |
|---|---:|---|---|
| `apps/desktop/src/App.tsx` | 4,445 | Application shell, navigation, global clipping flush ref, quit events | Must become smaller; no new durability state machine |
| `NewspaperClippingDetail.tsx` | 208 | Detail composition, save controller binding, conflict UI | May compose bounded hooks; must not own native exit protocol |
| `NewspaperClippings.tsx` | 107 | Gallery/detail selection and flush-before-switch | Keep as route coordinator only |
| `ClippingNoteEditor.tsx` | 453 | Tiptap document adapter, Markdown transactions, selection toolbar, composition | No durability implementation is added here |
| `clipping-note-slash-command.tsx` | 277 | Slash-command definitions, fuzzy ranking, suggestion renderer | No durability implementation; retain the enforced 300-line cap |
| `clipping-note-save-controller.ts` | 227 | Debounce, revisioned save, queued-latest state | Extend only the pure save state machine |
| `newspaper-api.ts` | 350 | Existing newspaper DTO and invoke facade | Do not turn into a durability API catch-all |
| `src-tauri/src/lib.rs` | 359 | Tauri assembly, tray, current cooperative Quit | Extract lifecycle ownership; keep assembly thin |
| `clipping_service.rs` | 3,694 | Provider service facade and crop/library behavior | Delegation only; no embedded draft repository or exit coordinator |
| `clipping_repository.rs` | 1,676 | Canonical clipping/search SQL | Only the atomic canonical-save/checkpoint-clear seam may change |
| `clipping_models.rs` | 979 | Existing clipping DTOs and errors | Do not add the recovery state machine here |
| `app/database.rs` | 3,577 | Shared schema lifecycle, backups, verification | Version bump plus bounded migration delegation only |
| `newspaper/commands.rs` | 665 | Thin Tauri newspaper commands | Add only thin checkpoint/recovery commands |
| `app/database_writer.rs` | 639 | Serialized accepted writes and shutdown drain | Reuse unchanged unless proof finds a real contract defect |

### 1.1 Confirmed strengths to preserve

- Canonical note updates use optimistic revisions.
- Only one canonical note save is in flight per controller.
- Edits made during an in-flight save become queued-latest work.
- Back, clipping switch, route navigation, and search takeover request a flush.
- Failed or conflicted flushes block those application-controlled transitions.
- The tray Quit path already has the beginning of a tokenized cooperative
  renderer handshake.
- Accepted backend writes are serialized and the database writer can drain
  before shutdown.

### 1.2 Confirmed gaps

1. The Windows main-window close request is not governed by the clipping flush
   handshake.
2. `RunEvent::ExitRequested` currently reaches service/database shutdown
   handling without first proving that the renderer draft is durable on every
   exit path.
3. Detail cleanup unregisters the flush callback and disposes the controller;
   React cleanup cannot await a final save.
4. The 800 ms trailing debounce has no maximum wait, so continuous typing can
   postpone canonical autosave indefinitely.
5. The controller rejects `setDraft` while conflicted even though the mounted
   editor can continue accepting visible edits.
6. There is no native recovery record for crash, forced termination, renderer
   failure, or power loss.
7. The current structural verifier proves cooperative tray Quit textually but
   does not prove close-X, updater, timeout, stale token, or database shutdown
   ordering.

## 2. Required behavior

### DUR-001: Graceful close has zero silent loss

For a mounted clipping editor, Back, clipping switch, route change, search
takeover, close-X, tray Quit, ordinary application exit, and updater-controlled
exit must all reach one durability decision before destroying the editor or
shutting down the database writer.

### DUR-002: Native code owns destructive lifecycle authority

The renderer reports state and performs saves. Rust decides whether a window
may close or the process may exit. Browser `beforeunload`, React cleanup, and a
fire-and-forget blur handler are supplementary signals, never the authority.

### DUR-003: Exit is fail-closed

If flush and the exact newest checkpoint both fail, a conflict has no matching
durable checkpoint, the mounted owner is lost, acknowledgement is stale, or the
bounded timeout expires, LinkVault remains running. The main window is shown
and focused with actionable recovery copy. A canonical revision conflict may
proceed only when the exact newest visible draft is checkpointed and explicit
recovery is guaranteed on next launch.

### DUR-004: Crashes recover a bounded checkpoint

Canonical notes remain in `newspaper_clippings`. A separate native SQLite
recovery record stores the latest coalesced draft. Search, list excerpts, FTS,
and ordinary note reads continue using canonical content only.

### DUR-005: Continuous typing becomes durable

Keep the approved 800 ms quiet-period canonical autosave and add a 5,000 ms
maximum canonical wait. A separate recovery checkpoint uses a 500 ms trailing
delay and a 2,000 ms maximum wait.

These are initial budgets, not permission to write on every transaction. If
release measurements show unacceptable database/WAL or typing cost, stop and
present measured alternatives before changing the budgets.

### DUR-006: Conflict retains the newest visible draft

While status is `conflict`, subsequent title and Markdown changes update the
local draft and recovery checkpoint but do not automatically overwrite the
canonical note. `Keep my changes` submits the newest visible draft against the
newly adopted revision.

### DUR-007: Recovery is explicit and revision-aware

When opening a clipping with a recovery record:

- matching `base_revision`: restore the draft and show `Recovered unsaved
  changes`;
- newer canonical revision: enter the existing conflict UI without replacing
  either body;
- byte-invalid/corrupt recovery record: keep the canonical note, report a safe
  diagnostic, and retain the recovery record for explicit cleanup;
- no record: open the canonical note unchanged.

### DUR-008: No draft data enters search early

Checkpoint writes do not update normalized metadata, FTS rows, gallery
excerpts, or `updated_at` on the canonical clipping. A canonical save continues
to update all existing search owners atomically.

## 3. Decisions and tradeoffs

### 3.1 Approved close-X behavior

**Approval:** Approved by the product owner on 2026-08-10.

Because LinkVault already owns a tray, close-X must:

1. synchronously prevent native close;
2. request flush/checkpoint;
3. hide the existing main window only after success;
4. keep the WebView mounted;
5. restore/focus the window on failure.

Tray **Quit** and explicit application exit must use the same preparation but exit
after acknowledgement. This avoids destroying and attempting to recreate the
only main WebView.

This resolves the earlier choice as `X means safely hide`; `Quit means safely
exit`. A later change requires a new product decision and must not silently vary
by Windows exit path.

### 3.2 Hard-crash loss window

Writing the full note synchronously on every key would reduce crash loss but
would make a 2 MiB editor stutter and grow SQLite WAL traffic. The recommended
checkpoint scheduler provides:

- zero silent loss for graceful lifecycle requests;
- normally no more than 500 ms of quiet-period work at risk during a hard
  crash;
- no more than 2 seconds of continuous-typing work at risk during a hard
  crash.

Reducing that hard-crash window requires measured incremental journaling and is
not part of this bounded slice.

### 3.3 Approved recovery-only size envelope

**Approval:** Approved by the product owner on 2026-08-10.

Canonical validation remains 800 title bytes and 2 MiB Markdown. Recovery must
also retain common invalid drafts, such as an empty title, because invalid
content is exactly when exit protection matters.

The absolute recovery-only envelope is 4 KiB of UTF-8 title bytes and 4 MiB of
UTF-8 Markdown bytes. At the recovery envelope, further edits stay visible but
exit is blocked until the user reduces, exports, or explicitly discards the
draft. These limits do not expand canonical-note validation or make an
oversized recovery draft searchable.

### 3.4 No automatic discard timeout

A timeout means `blocked`, not `discard`. No native fallback may confirm exit
merely because the renderer was slow or missing.

## 4. Module ownership and size budgets

The implementation must add bounded owners rather than growing existing
canonical files.

### 4.1 Frontend owners

| Path | Ownership | Soft/hard size budget |
|---|---|---:|
| `components/newspaper/clipping-note-save-controller.ts` | Pure canonical save state, 800 ms trailing timer, 5 s max wait, queued-latest, conflict draft capture | 280 / 340 lines |
| `components/newspaper/clipping-note-checkpoint-controller.ts` | Pure checkpoint coalescing, sequence/session identity, one in-flight checkpoint, 500 ms/2 s timers | 220 / 300 lines |
| `components/newspaper/clipping-note-durability-api.ts` | Checkpoint/recovery DTOs and thin Tauri/browser-harness invokes | 140 / 200 lines |
| `components/newspaper/useClippingNoteDurability.ts` | Compose save and checkpoint controllers for exactly one clipping detail | 180 / 260 lines |
| `components/newspaper/useClippingNoteExitBridge.ts` | Listen for tokenized native prepare requests, report surface state, acknowledge exact token | 180 / 260 lines |
| `NewspaperClippingDetail.tsx` | Render and bind the durability hook; conflict/recovery UI only | Current 208; hard maximum 260 |
| `NewspaperClippings.tsx` | Selection and pre-unmount flush only | Current 107; hard maximum 140 |
| `ClippingNoteEditor.tsx` | Tiptap document adapter only | No durability additions; hard maximum 500 lines |
| `clipping-note-slash-command.tsx` | Tiptap slash commands/suggestion UI only | No durability additions; hard maximum 300 lines |
| `App.tsx` | Mount the extracted exit bridge and retain navigation calls | Must be net-negative; target at most 4,410 lines |
| `newspaper-api.ts` | Existing content API | No durability additions |

Rules:

- No new production `.tsx` file may exceed 300 lines.
- A hook may coordinate owners but may not reproduce controller logic.
- Tiptap imports remain exclusive to the two explicitly budgeted editor owners
  above and the isolated editor-evaluation harness.
- Do not add a `ClippingNoteManager.tsx`, `NewspaperClippingsProvider.tsx`, or
  other canonical component that owns UI, persistence, native events, and
  recovery together.
- If a new module reaches its hard budget, stop and split by responsibility
  before adding behavior.
- Test files may exceed production budgets only when a scenario table is still
  clearer than another harness; split a test file at 500 lines.

### 4.2 Native owners

| Path | Ownership | Soft/hard size budget |
|---|---|---:|
| `app/cooperative_exit.rs` | Exit-attempt state, request token, deduplication, timeout, confirmation, blocked result | 240 / 320 lines |
| `app/database_migrations/mod.rs` | Bounded migration dispatch seam | 80 / 120 lines |
| `app/database_migrations/newspaper_clipping_drafts.rs` | Schema-v6 DDL and verifier for recovery drafts | 220 / 300 lines |
| `providers/newspaper/clipping_draft_repository.rs` | Recovery SQL only; no Tauri or UI types | 240 / 340 lines |
| `providers/newspaper/clipping_draft_service.rs` | Validation envelope, writer submission, recovery classification, canonical-clear inputs | 260 / 360 lines |
| `providers/newspaper/commands.rs` | Thin checkpoint/load/discard commands | Maximum net addition 80 lines |
| `providers/newspaper/clipping_service.rs` | Delegate to the draft service | Maximum net addition 30 lines |
| `providers/newspaper/clipping_repository.rs` | Clear the matching acknowledged checkpoint inside the existing canonical save savepoint | Maximum net addition 25 lines |
| `app/database.rs` | Schema version 5 to 6 and migration call | Maximum net addition 25 lines |
| `src-tauri/src/lib.rs` | Assemble extracted exit owner and route close/exit events | Must not exceed current 359 lines |

Rules:

- `lib.rs` contains assembly and event routing, not mutex/condition-variable
  implementation.
- `clipping_service.rs` does not gain SQL or timer state.
- `clipping_repository.rs` remains canonical clipping/search SQL; recovery CRUD
  belongs in its sibling repository.
- `database.rs` remains the transaction/backup authority but delegates v6 DDL
  and verification.
- All ordinary commands remain path-free and return typed safe error codes.
- Do not add a second database writer, connection pool, async runtime, or
  background polling thread.

### 4.3 Structural size gate

Add `scripts/verify-clipping-note-durability-structure.mjs` to fail when:

- a listed hard line budget is exceeded;
- `App.tsx`, either production Tiptap owner, `clipping_service.rs`, or
  `clipping_repository.rs` contains the new controller implementation;
- recovery invokes are added to `newspaper-api.ts`;
- Tiptap imports escape the editor adapter;
- `localStorage`, `sessionStorage`, `beforeunload`, or `pagehide` is used as
  durability authority;
- draft SQL appears outside migration/draft repository/canonical atomic-clear
  seams;
- close or exit paths call database shutdown before confirmed durability.

The script is a guardrail, not a substitute for a code review of ownership.

## 5. Recovery persistence contract

### 5.1 Schema v6

Approved table shape:

```sql
CREATE TABLE newspaper_clipping_note_drafts (
  clipping_id TEXT PRIMARY KEY
    REFERENCES newspaper_clippings(id) ON DELETE CASCADE,
  base_revision INTEGER NOT NULL CHECK(base_revision >= 1),
  writer_session_id TEXT NOT NULL,
  writer_sequence INTEGER NOT NULL CHECK(writer_sequence >= 1),
  draft_title TEXT NOT NULL,
  draft_markdown TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
```

The migration must follow the existing database backup, transaction,
`user_version`, schema-verification, future-version rejection, and rerun
contracts. No FTS trigger is added for this table.

### 5.2 Checkpoint identity

Each mounted detail creates one unguessable `writer_session_id` and a monotonic
`writer_sequence`.

- Same session: only a higher sequence may replace the row.
- Different session with an unresolved row: do not overwrite it silently;
  return a recovery/conflict result.
- A newly opened editor must load/classify an old row before claiming it.
- Stale async completion cannot clear a newer sequence.

### 5.3 Atomic canonical save and clear

The update request carries the acknowledged writer session and submitted
sequence. Within the existing canonical note savepoint:

1. verify expected canonical revision;
2. update title, Markdown, revision, normalized metadata, and FTS;
3. delete only the recovery record whose session matches and whose sequence is
   less than or equal to the canonical submission;
4. release the savepoint.

On revision conflict, validation error, SQL error, or process interruption, the
recovery record remains. A no-op canonical update may clear the matching
checkpoint only after proving its bytes equal canonical bytes.

### 5.4 Explicit discard

Discard is a separate typed command requiring clipping ID, writer session, and
current sequence. Navigation, timeout, unmount, and startup cleanup never call
discard implicitly.

## 6. Frontend state and event protocol

### 6.1 Surface state

The renderer reports one of:

```text
clean
dirty
saving
checkpointing
failed
conflict
recovered
```

`clean` means canonical bytes are acknowledged and no newer checkpoint is
pending. A successful checkpoint alone does not make the canonical note clean.

### 6.2 Native preparation request

```text
native prevents close/exit
  -> prepare request { token, reason, deadline }
renderer verifies current mounted clipping identity
  -> flush canonical queued-latest work
  -> if canonical flush cannot succeed, ensure recovery checkpoint is durable
  -> acknowledge exact token with canonical/checkpoint outcome
native accepts only current token
  -> hide for close-X, or confirm process exit
```

An acknowledgement must include enough information to distinguish:

- canonical clean;
- recovery checkpoint durable but canonical save blocked;
- no mounted clipping editor;
- failed, uncheckpointed/stale-conflict, timed-out, or unmounted.

The recommended default allows process exit only for `canonical clean` or
`recovery checkpoint durable`. Product copy must say `Recovered draft will be
available next time` when exiting on a checkpoint rather than a canonical save.

### 6.3 In-flight and queued-latest ordering

`flush()` must:

1. cancel both debounce timers;
2. await the submitted canonical save;
3. re-read controller state;
4. save the queued-latest draft if it differs;
5. wait for its matching recovery clear;
6. return success only for the newest visible generation.

Do not cancel an accepted database write. Ignore stale UI acknowledgements by
document/session/generation identity.

### 6.4 Unmount

React cleanup may unregister listeners only after the route/native coordinator
has reached a durable outcome. If an unexpected unmount occurs first, native
surface removal resolves an active preparation request as blocked. The latest
already acknowledged recovery checkpoint remains available at restart.

## 7. Native lifecycle ordering

### 7.1 Main-window close-X

1. `WindowEvent::CloseRequested` calls `prevent_close()` immediately.
2. Deduplicate against an active lifecycle attempt.
3. Request clipping durability from the main renderer.
4. On success, hide the existing main window.
5. On blocked/timeout, show and focus it and emit actionable UI state.

### 7.2 Tray Quit / application exit

1. `RunEvent::ExitRequested` calls `prevent_exit()` unless a confirmed token is
   being consumed.
2. Keep `DatabaseWriter` accepting work while the renderer flushes.
3. Await the current clipping durability result.
4. On success, mark exactly one confirmed exit and call `app.exit(0)`.
5. On the confirmed exit event, stop new crop work, drain accepted crop work,
   then drain/shut down the database writer.
6. On failure, clear the active attempt and keep the application alive.

The existing unconditional shutdown reaction to an unconfirmed
`ExitRequested` must not remain.

### 7.3 Updater and restart

Updater installation must use the same preparation owner, including the Tauri
Windows before-exit hook where required. It may not bypass durability by
calling a direct process exit.

### 7.4 Forced operating-system termination

The application cannot promise an async handshake after Windows forcibly ends
the process. Safety for that case comes from the already durable recovery
checkpoint and atomic SQLite writes, not from pretending `beforeunload` is
reliable.

## 8. Performance and resource budgets

### 8.1 Interaction path

- No synchronous filesystem, SQLite, invoke wait, hashing of the full document,
  or JSON storage write occurs inside the Tiptap transaction handler.
- Editor transactions update in-memory references and schedule bounded work.
- At most one checkpoint invoke and one canonical save are in flight per
  clipping.
- Queues retain only the newest pending snapshot, never an unbounded history.
- Composition continues to emit only committed text.

### 8.2 Write frequency

For ten seconds of uninterrupted typing:

- recovery checkpoints: at most five submissions with a 2 s max wait;
- canonical saves: at most two submissions with a 5 s max wait;
- after typing stops: at most one final checkpoint and one final canonical save,
  with redundant byte-identical work suppressed.

Tests use a fake clock for exact counts. A real release-mode collector verifies
that the measured implementation follows the same bound.

### 8.3 Bundle and startup

- No new runtime dependency is permitted.
- The durability bridge loads with the clipping detail, not other routes.
- Main-route raw and gzip JS must not regress by more than the small measured
  bridge delta; record before/after sizes rather than inventing an allowance.
- The already lazy Tiptap chunk must not grow from durability work.
- Startup does not scan or deserialize every recovery draft. Query by selected
  clipping ID; a cheap count may be used only for a recovery indicator.

### 8.4 SQLite/WAL

- Reuse `DatabaseWriter`; do not open a competing write path.
- Upsert one row per clipping rather than append full snapshots.
- Do not force a WAL checkpoint per edit.
- Measure database/WAL growth during a ten-minute continuous-typing workload
  with a near-limit note.
- Preserve the existing persistence baseline. If contention or write latency
  regresses more than 10% across three clean release measurements, stop and
  investigate before relaxing the gate.

### 8.5 Memory

- Do not retain every draft generation.
- Avoid `localStorage`/`sessionStorage` copies of a near-2 MiB note.
- Release submitted strings after acknowledgement.
- Record browser heap and native working-set deltas for 8, 50, and 500 clipping
  libraries with only one mounted editor; library size must not multiply active
  draft memory.

## 9. Implementation slices

Each slice must pass its focused gates before the next begins.

### Slice A: Pure controller correctness

- Add maximum-wait semantics to the existing save controller.
- Continue capturing draft changes during conflict.
- Add the pure checkpoint controller.
- No Tauri, schema, or UI changes.

### Slice B: Schema and native checkpoint service

- Confirm the approved decision/spec amendment, schema v6, and recovery size
  envelope.
- Add bounded migration modules.
- Add draft repository/service and thin commands.
- Make canonical save clear only its acknowledged checkpoint atomically.
- No close/exit changes yet.

### Slice C: Detail integration and recovery UI

- Add durability API and hook.
- Load/classify/claim recovery before enabling edits.
- Render recovered/conflicted copy using the existing detail conflict surface.
- Keep `ClippingNoteEditor.tsx` unchanged.

### Slice D: Native close and exit authority

- Extract the current cooperative Quit implementation from `lib.rs`.
- Cover close-X, tray Quit, `ExitRequested`, updater exit, stale token,
  duplicate request, timeout, and blocked state.
- Correct crop/database shutdown ordering.
- Extract the App quit listener into the bounded exit bridge hook.

### Slice E: Performance, native UAT, and release evidence

- Run clean release collectors.
- Run installed/native close and recovery scenarios.
- Update the decision register, specifications, README test instructions, and
  PR evidence with actual results.

Do not combine all slices into a single unreviewed commit merely because they
share one work order.

## 10. Focused automated tests

### 10.1 Pure frontend controller matrix

- 799 ms no-save and 800 ms trailing save.
- Continuous typing forces canonical save at 5 s.
- Checkpoint trailing and max-wait counts.
- One in-flight save plus repeated edits persists only queued-latest.
- Flush during in-flight work waits for the newest generation.
- No-op return to canonical bytes avoids a redundant write.
- Conflict retains edits made after the conflict appears.
- `Keep my changes` submits the latest visible conflict draft.
- Save failure preserves both draft and checkpoint.
- Stale document/session/sequence acknowledgements are ignored.
- Disposal cannot report success for unacknowledged work.
- Empty title, over-limit title/note, and null input follow canonical versus
  recovery envelope policy.

### 10.2 Rust migration/repository/service matrix

- Fresh v6 schema.
- v5 to v6 migration with verified backup.
- Migration failure leaves `user_version` below 6 and original data readable.
- Rerun is a no-op.
- Future version fails closed.
- Upsert accepts increasing sequence and rejects stale sequence.
- Different unclaimed writer cannot overwrite recovery.
- Canonical successful save clears only matching acknowledged recovery.
- Canonical no-op clears matching identical recovery.
- Conflict/failure leaves recovery unchanged.
- Clipping deletion cascades recovery deletion only through the existing
  authorized clipping deletion transaction.
- Draft writes do not mutate FTS, metadata, revision, or canonical `updated_at`.
- Safe errors contain no note content, title, filesystem path, or SQL.

### 10.3 Native lifecycle matrix

- Clean close-X safely hides once.
- Dirty close-X waits for canonical flush.
- Failed close-X remains visible.
- Checkpoint-durable/canonical-failed policy matches the approved decision.
- Tray Quit waits for queued-latest.
- `ExitRequested` cannot shut down the writer before renderer acknowledgement.
- Duplicate X/Quit creates one active request.
- Stale token cannot confirm a newer attempt.
- Renderer unmount during request blocks exit.
- Timeout blocks exit and restores focus.
- Confirmed exit drains accepted note then crop writes before writer shutdown.
- No mounted clipping editor exits without a needless delay.
- Updater exit uses the same coordinator.

### 10.4 Browser integration matrix

- Type and immediately Back.
- Type and immediately switch clipping.
- Type and immediately open search.
- Edit title and body during an in-flight acknowledgement.
- Conflict, continue typing, then keep local or use saved version.
- Simulated native prepare event and exact acknowledgement.
- Recovery reopen with matching and changed base revisions.
- No draft content appears in gallery search until canonical save.
- Strict Mode mount/unmount produces one active registration.
- No console/page error or external request.

## 11. Native Windows UAT

Automated browser proof is not close/exit proof. On the packaged or native dev
window, record:

1. Type and click X before 800 ms; reopen from tray and confirm exact text.
2. Type continuously, click X while `Saving`, and confirm queued-latest.
3. Type and immediately choose tray Quit; restart and confirm canonical or
   explicitly recovered state.
4. Simulate a typed save failure and confirm X/Quit does not silently discard.
5. Trigger a revision conflict, type more, close, reopen, and confirm the newest
   visible draft is recoverable.
6. End the process after a checkpoint using the approved crash harness, restart,
   and confirm the recovery banner and exact bytes.
7. Repeat title/body close with a native Chinese IME composition commit.
8. Verify normal close-to-tray, Show, second close, and final Quit do not create
   duplicate windows or stale exit attempts.

Do not use Task Manager termination against an uncommitted user database. The
crash-recovery UAT must use an isolated test database/profile.

## 12. Required gates

Run focused gates first and stop on the first ownership or data-loss defect.
The existing `verify:clipping-note-autosave` command is retained. The
`verify:clipping-note-durability-structure` and
`verify:clipping-note-durability-browser` commands are implementation
deliverables and must be added to `apps/desktop/package.json`; their absence
before Slice A is expected and is not evidence that the durability work is
complete.

```powershell
npm.cmd --prefix apps\desktop run verify:clipping-note-autosave
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-structure
npm.cmd --prefix apps\desktop run verify:clipping-note-durability-browser
npm.cmd --prefix apps\desktop run verify:newspaper-clipping-library
npm.cmd --prefix apps\desktop run verify:newspaper-clipping-library-browser
```

Native focused gates must include the new draft/migration/exit test filters
under the Visual Studio developer environment.

After focused proof:

```powershell
npm.cmd --prefix apps\desktop run build
npm.cmd --prefix apps\desktop run verify:architecture
npm.cmd --prefix apps\desktop run verify:persistence
npm.cmd --prefix apps\desktop run verify:ui
npm.cmd --prefix apps\desktop run verify:newspaper-clippings
npm.cmd --prefix apps\desktop run verify:release
npm.cmd --prefix apps\desktop audit --omit=dev
git diff --check
```

Also run:

- `cargo fmt --check`;
- `cargo clippy --all-targets`;
- full Rust tests;
- release-mode durability performance collector;
- source audit for new `#[ignore]`, time-based sleeps, unsafe error content,
  direct write connections, and size-budget exceptions;
- generated-output and owned-port/process cleanup.

No ignored durability test is accepted. A manual collector may be explicitly
ignored only when the release gate invokes it by exact name and records output.

## 13. Performance report

Record before/after on the same clean commit and machine:

| Measurement | Required evidence |
|---|---|
| Main and lazy editor bundle raw/gzip | Normal production build output |
| Checkpoint schedule/submit count | 10 s and 10 min continuous typing |
| Canonical save count | Same workloads |
| Checkpoint p50/p95/max latency | 1 KiB, 100 KiB, 2 MiB drafts |
| Exit handshake p50/p95/max | Clean, dirty, in-flight, checkpoint fallback |
| SQLite/WAL growth | Near-limit 10 min workload and post-idle state |
| Persistence baseline | Existing release collector before/after |
| Memory | One active editor with 8/50/500 clipping libraries |

If a threshold is not already approved, record the baseline first. Do not make
a failing measurement green by inventing a looser limit after implementation.

## 14. Compatibility and conflict audit

Before implementation closeout, re-check:

- `App.tsx` navigation queue still blocks only on real durability failures.
- Search continues to read canonical FTS only.
- Gallery thumbnail lazy loading is unaffected.
- Tiptap composition, slash commands, task lists, and selection toolbar remain
  editor-owned.
- Reader crop/save and `Open note` do not wait on unrelated checkpoint work.
- Snapshot roots and clipping assets remain path-independent from notes.
- Canonical note update retains the current optimistic revision response.
- Database writer shutdown still drains all accepted provider work.
- Tray Show restores and focuses the same main WebView because close-X hides
  rather than destroys it.
- Updater installation cannot bypass the exit coordinator.
- No draft bytes appear in logs, diagnostics, toast error details, or PR
  evidence.

## 15. Stop rules

Stop and request a decision if implementation would:

- require a new runtime dependency;
- add durability logic to `ClippingNoteEditor.tsx`;
- increase `App.tsx`, `clipping_service.rs`, `clipping_repository.rs`, or
  `database.rs` beyond the listed delta budget;
- make recovery drafts searchable;
- discard on timeout or unmount;
- permit database shutdown before note durability acknowledgement;
- require synchronous full-note work in the typing transaction;
- deviate from the approved `X hides; Quit exits` semantics without a new
  product-owner decision;
- change the approved 4 KiB title / 4 MiB Markdown recovery envelope without a
  new product-owner decision;
- weaken optimistic revision checks;
- claim hard-crash zero-loss without a measured synchronous/incremental journal.

## 16. Rollback boundary

Rollback is bounded by the v6 migration and feature wiring:

- Removing UI/native exit wiring leaves canonical note saves functional.
- Recovery commands may be disabled without deleting v6 recovery rows.
- Never downgrade `user_version` or drop recovery data automatically.
- A forward repair may clear a recovery row only after canonical equality or
  explicit user discard is proven.
- The implementation PR must document how to disable the new coordinator while
  preserving recoverable drafts.

## 17. Completion definition

This durability slice is complete only when:

- close-X and every exit path are native-gated;
- continuous typing has a bounded canonical save interval;
- conflict continues capturing the visible draft;
- recovery checkpoints survive renderer/process loss;
- canonical save and checkpoint clear are atomic and sequence-safe;
- canonical search never indexes drafts;
- all module size budgets and structural ownership checks pass;
- release performance evidence shows bounded writes, memory, WAL growth, and
  no unacceptable persistence regression;
- native Windows close, Quit, restart, conflict, and crash-recovery UAT is
  recorded separately from browser proof;
- no generated output, test server, or unrelated process remains;
- the worktree contains only the audited durability and already-approved
  clipping editor changes.
