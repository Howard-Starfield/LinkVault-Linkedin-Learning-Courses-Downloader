# Newspaper native scheduling and seven-day catch-up

## Behavior and scope

Enabled schedules reconcile individual edition/publication-date gaps at native
startup, schedule changes, deadline wakes, and a bounded sleep/clock recovery
check. The calendar window is today minus six days through today; today's
publication becomes eligible only after its configured local schedule time.
Publication cadence and dated special editions remain provider-owned.

Automatic reconciliation preserves every recorded attempt, including queued,
paused, dismissed, failed, unavailable, cancelled, partial and completed work,
across destinations. It inventories completion markers only at bounded candidate
paths under known schedule destinations (including disabled destinations);
it does not recursively discover arbitrary unindexed archives. Existing missing
or inaccessible downloaded files use explicit library recovery/repair. Older
dates outside this window use manual range downloads.

A rolling per-schedule checkpoint in existing `newspaper_settings` remembers
observed initiated dates so deleting observed queue/history entries does not
immediately cause automatic recreation. It stores no credentials, uses no new
schema, and is ignored by older versions. Reconciliation is idempotent and skips
unchanged checkpoint/watermark writes. Removing a schedule removes its checkpoint
and cancels its owned pending work atomically.

## Ownership

- `schedule_service`: bounded filesystem preflight outside the writer; final
  configuration and identity recheck inside a DatabaseWriter transaction.
- `batch_service`: manual producer uses the same writer boundary, with uncapped
  workflow identity checks. Batch, native runs, steps and audit events commit
  together using the repository's caller-owned transaction helper.
- `workflow/application/runtime`: existing supervisor owns wake generations,
  deadlines, bounded per-type workers and shutdown joins. Generic hooks keep
  provider code out of the kernel. Locally executing run IDs are exempt from
  lease expiry; unrelated stale runs still recover.
- `newspaper/supervisor`: provider planning/deadlines and compatibility work
  adapt to the shared owner. Kernel and legacy downloads share admission;
  optimization may overlap downloads. Unexpected service failures back off.
- `executor` and queue activation: materialization and eligibility checks use
  the writer. Paused work retains a resumable compatibility row, and legacy
  selection excludes nonterminal native-owned jobs.
- `commands`: submission/mutation wakes are explicit; interactive SQLite and
  filesystem waits run off the UI executor. Optimization command replies use
  a one-shot response while the shared runtime owns its worker lifetime.
- `App`/`NewspaperView`: no recurring renderer queue driver; listeners precede
  initial activity reads. Idle snapshots stop. Visible active downloads retain
  a one-second snapshot fallback; focus/visibility and invalidations recover
  state, and failed reads retry with a bounded delay.

The existing compatibility page persistence and archive/job service writes are
not a full provider persistence cutover. Lexical gate counts include writer
submissions and transaction starts; reviewed increases do not authorize new
runtime writer connections. Baseline version 22 records the exact affected
owners.

## Validation boundary

Focused regressions cover seven-day/cron/cadence eligibility, gaps in the middle,
all-state/global inventory deduplication, history beyond 1,000 runs, concurrent
submission and rollback, deletion, wake races, long worker scheduling, future
pause/resume, compatibility double-claim prevention, optimization retry/lease
deadlines, cancellation gates, idle UI behavior and disposal.

Native validation uses an isolated copy with schedules disabled and no runnable
downloads. Real publisher catch-up downloads and physical Windows suspend/resume
are separate UAT; synthetic deadline/clock fixtures do not prove those external
conditions. Installed software and original user data remain outside this change.

## Verified results

- Full Rust suite: 906 passed, 0 failed, 6 ignored. Canonical persistence
  gate: 59 passed. Clippy: passed with the existing 38 library / 56 library-test
  warnings, with no increase.
- Release persistence benchmark: 800/800 writes completed, zero failed writes,
  356 ms contention run and 1 ms snapshot read; diagnostic checks passed.
- TypeScript build and no-any check passed (71 project-owned files). Architecture,
  UI, polling/queue ownership, snapshot equality and virtualization checks passed.
- Current mocked browser profile passed; all ten reader controls remained visible
  and reachable at nine widths from 320 through 1,720 pixels, without overlap or
  horizontal overflow. Native reader toolbar and clipping enter/cancel also passed.
- Native isolated copy: 82 completed editions, no live work. Over 32.015 seconds
  the newspaper screen issued zero idle IPC calls and received zero queue/library
  invalidations. Native scrolling/navigation measured 6,647 animation frames,
  a maximum 48.5 ms frame gap, no gaps above 100 ms and no reported long tasks.
- Original database, encrypted token and history hashes stayed unchanged. The
  copied database passed integrity and foreign-key checks after shutdown.

Evidence: `.tmp/stutter-validation-20261003/newspaper-native-scheduling.json`,
`scheduling-reader-trace.json`, `scheduling-source-after.json`, and
`scheduling-copy-integrity.json`. These local measurements are bounded proof for
this build and fixture; they do not establish all-device performance or real
network catch-up acceptance.

## v0.2.28 release preparation

All application/UI/lockfile versions advance together to 0.2.28. The release
manifest gate now checks both npm lockfile identities and the Cargo app lock
record. Idle-activity and queue-owner fixtures are included in the release gate.

The pre-release audit identified Tiptap Markdown/attribute advisories. Its six
direct packages and existing transitive Tiptap family are pinned/aligned to
3.31.4; the required ProseMirror model/view dependencies and the Nano ID build
dependency receive narrow updates. Other dependency versions are preserved. The
full npm dependency audit reports zero vulnerabilities. The editor's 17 browser
checks, normalization/autosave/lifecycle checks, frontend build, no-any and version
verification passed. The general visual verifier was updated to the current
newspaper groups while retaining containment/overlap/hit-target/overflow,
sidebar, native-floor and keyboard checks; it passed against the local frontend.

The repository's existing tag-triggered Windows workflow remains the installer
and updater-signing owner. No user-data migration or installed-app update is
performed by release publication.

The canonical v0.2.28 release gate passed on the final source: 906 Rust tests
passed, six ignored; 59 persistence gates passed. Its release benchmark completed
800/800 writes with zero failures (403 ms contention, 1 ms snapshot).
The release gate also passed the version/lock consistency, frontend build, UI,
architecture, activity equality, poll scheduling and queue ownership checks.
