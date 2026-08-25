# ADR-003: YouTube V1 transient workflow bridge

**Status:** Accepted (original); proposed amendment dated 2026-08-23,
pending coordinated PRD update and owner-risk re-acceptance

**Original decision date:** 2026-08-20

**Amendment note:** This is an internal architecture contract amendment. It does
not claim public release, public packaging, platform permission, legal/counsel
approval, or renewed owner-risk acceptance. Material reviewed-specification or
user-facing-scope changes still require the applicable owner-risk re-review;
`Y-PUBLIC-REVIEW` remains a separate gate. The catalog permission below is
proposed and is not implementation authorization until the coordinated PRD
update and owner-risk re-acceptance are recorded.

**Decision owners:** Howard Deng and LinkVault engineering

**Related decisions:**

- [ADR-001: Unified workflow modular monolith](adr-001-unified-workflow-modular-monolith.md)
- [Unified workflow migration plan](unified-workflow-migration-plan.md)
- [YouTube Downloader V1 PRD](../specs/youtube-downloader-v1.md)

## Context

ADR-001 assigns scheduling, lifecycle transitions, cancellation, resource
governance, progress revisions and shutdown tracking to the shared workflow
kernel. The current repository has established that ownership boundary but has
not implemented the Phase 2 durable runtime.

YouTube V1 needs a bounded external-process supervisor before the durable
kernel is available. Giving that supervisor to the YouTube provider would
create the fourth provider-local lifecycle engine prohibited by ADR-001.
Waiting for every durable workflow feature would instead couple a deliberately
narrow provider slice to unrelated persistence and migration work.

The product also needs a cross-restart YouTube Downloads view for artifacts
that have already been verified and published. That view requires a small
durable read projection, but it does not require durable workflow execution
state. This amendment defines that projection and its recovery boundary.

## Decision

LinkVault will introduce a workflow-owned, non-durable transient bridge for
YouTube V1.

```text
React YouTube view
  -> typed Tauri command adapter
  -> TransientWorkflowRuntime
  -> YouTube planner/executor adapter
  -> managed yt-dlp / FFmpeg process tree
```

`TransientWorkflowRuntime` is created once in the Tauri composition root and
lives under `crate::workflow::transient`. It owns:

- opaque run identity and monotonic in-memory revisions;
- the transient run transition matrix;
- a bounded revision/event stream plus reconstructable current/most-recent
  snapshot;
- pause-after-current-item admission;
- cancellation tokens and terminal-state arbitration;
- the bounded set of managed external-process trees;
- Windows Job Object containment and tracked application shutdown; and
- the atomic discovery/run/shutdown admission state and global caption-helper
  semaphore used by this bridge.

The YouTube provider owns:

- URL validation and discovery;
- immutable scan-plan construction;
- playlist occurrence identity and selection validation;
- transcript-track normalization and deterministic selection;
- typed yt-dlp/FFmpeg argument construction;
- provider error classification;
- output naming inputs and provider metadata; and
- media, transcript and manifest verification before publication.

The app-owned safe-filesystem service owns validated output-root handles,
reparse-point defense, clean staging creation and atomic publication. The
provider supplies only typed relative-path and artifact plans.

The bridge is intentionally not the durable Phase 2 kernel. It MUST NOT add:

- workflow, job, event, lease, schedule or retry database tables;
- automatic restart recovery or cross-restart continuation;
- provider-specific domain branches inside workflow code;
- a generic shell or sidecar command exposed to React;
- a background processing or polling loop owned by React; or
- reuse by another provider without an amendment to this ADR.

The proposed amendment would permit the bridge to publish a verified YouTube
artifact into the versioned, provider-domain catalog defined below. The catalog
is a read projection of completed, verified artifacts, not a workflow engine.
It MUST NOT contain or become a source of truth for workflow, job, event,
lease, schedule, retry, active-progress, transient-snapshot, process, or
submission state. In particular, transient state is never dual-written to the
catalog or to any durable workflow table.

V1 has one mutex-protected admission state:

```text
Idle | Discovering(operation_id) | Running(run_id) | ShuttingDown | Quarantined
```

Scan, transcript inspection and start acquire their state atomically before
helper verification or network work. A matching RAII guard releases
`Discovering`; an abandoned or panicking command cannot strand the state. Each
frontend operation ID is single-use for the process lifetime and retained in a
bounded no-eviction tombstone set, so a delayed cancel cannot target a reused
ID. Start
atomically pins its immutable run plan before publishing `Running`. Discovery
uses one flat-scan helper and one global semaphore permits at most two caption
helpers across the whole runtime. Shutdown atomically prevents all later
admission and cancels the exact discovery operation or run. `Quarantined`
rejects work after a native cleanup result cannot be established. A download run
executes one selected occurrence and one managed helper tree at a time. These
are product-level serialization/safety rules for the bounded V1 contract, not
tuned claims about optimum concurrency. Release-build measurement is required
before increasing them.

## Dependency direction

The workflow bridge defines provider-agnostic transient lifecycle types and a
Rust-only managed-process port. It MUST NOT import or re-export
`providers::youtube`. Provider-module aliases may not be used to evade this
direction.

The YouTube adapter depends on the public transient port. The composition root
registers the adapter and runtime. Tauri commands remain thin typed adapters;
the frontend cannot submit executable paths or argument vectors.

## Lifecycle contract

The manager has no run while idle. A successful start creates a run in
`running`; run transitions are:

```text
running -> pause_requested -> paused -> running
pause_requested -> running
running -> cancelling -> cancelled
pause_requested -> cancelling -> cancelled
running | pause_requested -> completed | completed_with_warnings | failed
paused -> cancelling -> cancelled
```

Terminal states are immutable. A cancellation request accepted before terminal
commit wins over later helper success. Every accepted mutation increments the
run revision. Commands targeting a stale or different run fail without
affecting the current run.

Pause means no next playlist occurrence is admitted after the current
occurrence reaches a safe boundary. Resume is valid from `paused`; before the
safe boundary it may also withdraw `pause_requested` back to `running`.

## Shutdown contract

Closing the window to the tray does not stop a run. `CooperativeExit` remains
the sole app-owned exit authority and is extended to compose distinct renderer
durability and native-shutdown participants for one token. The YouTube runtime
MUST NOT call the frontend resolution command or authorize exit independently.

For `Close`, only the existing renderer-durability participant runs and a
successful result hides the window. For tray Quit, ordinary process exit and
updater restart, the same token first obtains renderer durability and then
runs registered native shutdown participants in deterministic order. The
YouTube participant MUST:

1. stop new discovery and item admission;
2. request cancellation of active discovery or run work;
3. terminate every managed process tree through its Job Object;
4. drain bounded stdout/stderr readers;
5. await cleanup for a bounded interval; and
6. return its participant result only after cleanup completes.

The native barrier phases are `Open -> Quiescing -> Draining -> Closed` or
`Blocked`. Required participants register in the composition root before
`Quiescing`; late registration is rejected. Each has a unique token and resolves
once. Concurrent Exit requests share the same attempt token and aggregate
result. Caption permits, publication handles and managed process handles remain
owned by their participant through join and cannot leak across `Closed`.

`CooperativeExit` authorizes process exit exactly once only after every required
participant for that token succeeds. A stale, failed or timed-out renderer or
native result cannot be overwritten by another participant. A forced Job
Object termination is still followed by handle/reader joins and verified
absence of owned children; if that bounded cleanup cannot be established, the
runtime commits `Quarantined`, `APP_SHUTDOWN_TIMEOUT` blocks a clean-exit claim
and the main window is restored with a restart-required explanation. The
blocked attempt token is retired. A later Exit uses a fresh token and may rerun
idempotent cleanup verification; success exits, failure remains quarantined.
Renderer failure before native drain leaves the prior runtime state unchanged.
Updater installation/restart must
enter this same `Exit` path and cannot call `exit` around the barrier.

The process is created and assigned to a kill-on-close Job Object before it is
allowed to create descendants. A test-only process adapter must prove child and
grandchild termination; killing only the immediate yt-dlp PID is insufficient.

## Artifact contract

The bridge does not create a generic durable artifact repository. Start pins a
run-owned immutable plan that survives expiry of the discovery cache for the
life of that run. The run plan/audit fingerprint is distinct from the stable
per-item artifact fingerprint. The latter contains only canonical occurrence
and video identity, effective mode, format policy, transcript selection and
helper-lock version; it excludes run ID, discovery-cache ID, whole-run
selection and output root.

The app safe-filesystem service creates a new unpredictable, exclusive staging
directory for every attempt. Preserved partials are never exposed to a helper
in place: regular files with a matching item fingerprint are opened without
following reparse points, identity-checked and copied into the clean attempt.
Every helper-visible descendant is rejected if it is a symlink, junction or
other reparse point, and opened artifact identity is rechecked before hashing
and publication.

Files become visible as a completed item only after provider verification and
atomic publication. Incompatible or identity-mismatched partials are not
silently reused.

### Verified YouTube artifact catalog amendment

If this proposed amendment is approved, the application MAY maintain one
versioned provider-domain read projection for verified YouTube artifacts so a
Downloads tab can survive an application restart. Versioning is part of the
catalog contract; a future schema change requires an explicit migration and
compatibility decision. A V1 catalog record contains only bounded, verified
artifact information:

- provider and catalog schema version;
- stable artifact/manifest identity and checksum projection;
- a backend-created safe artifact locator;
- bounded display metadata such as title, channel and acquisition time;
- media/transcript artifact summary; and
- a bounded warning summary.

Every row originates from a successfully verified published manifest. A later
revalidation may retain the row with inert `missing` or `corrupt` status so the
UI can explain loss or tampering, but such a row grants no reveal, reuse or
publication authority. Only fresh safe-root and manifest re-verification may
restore `verified` status.

It does not contain `runId`, `scanPlanId`, submission identity, workflow state,
event history, lease, retry, schedule, active progress, executable or command
data, or an unverified partial. Removing a catalog row does not remove media;
media deletion, if later offered, is a separate explicit and confirmed
operation.

The safe locator is a typed, versioned value created by the app-owned
safe-filesystem service after publication. It identifies the registered
YouTube output root by an opaque root identity and a normalized relative item
path, together with the expected manifest identity/version. It MUST NOT carry
an absolute path, arbitrary filesystem path, executable path, URL, signed URL,
or argument vector. A backend reveal/open operation re-resolves the registered
root, rejects absolute/traversal paths and reparse points, verifies root and
file identities, and re-validates the expected manifest before returning a
capability to the native open/reveal owner. React may retain or return the
locator, but cannot construct one or open a path directly. Any failed
resolution is fail-closed; a display-relative path is derived by the backend
and is not an authority to access the filesystem.

The application persistence layer owns the durable YouTube output-root
registry and catalog migration, using the existing initialization/migration
path and `DatabaseWriter` for state-changing transactions. The provider may
request registration and resolution through app-owned ports but does not open
an independent database connection or create a second writer. A user-selected
directory is accepted only as untrusted initial root-admission input; after
validation, restart-visible operations use the opaque root identity. On
restart, the safe-filesystem owner reopens and revalidates the stored root
identity. A missing, moved or replaced root remains disconnected until an
explicit identity-preserving reconnect; path text alone never silently rebinds
it.

Catalog publication has one required order:

1. Provider verification produces the bounded artifact metadata and manifest.
2. The safe-filesystem service atomically publishes the clean attempt inside a
   registered output root.
3. The published directory, manifest checksums and file identities are
   reopened and verified after publication.
4. Only after that verification succeeds, one app-owned database transaction
   inserts or updates the versioned provider-domain catalog record and its safe
   locator.

The transaction contains catalog projection changes only; it is not a commit
of a run or a workflow event. If publication succeeds but the catalog
transaction fails, the verified artifact remains in place, the item returns a
typed catalog-sync warning plus the publication locator, and a later
reconciliation may adopt the verified manifest. Reveal revalidates that
locator directly through the registered-root and manifest identities; catalog
presence is not required. The implementation MUST NOT report a catalog row before the
transaction commits and MUST NOT delete a verified artifact merely because
the projection transaction failed.

Recovery and reconciliation are limited to output roots explicitly registered
with the app-owned safe-filesystem service for YouTube. Startup or an explicit
list/reconcile operation may inspect paginated, hard-bounded candidate item directories in
those roots, but it MUST reject root replacement, reparse components, path
escape, malformed metadata, checksum mismatch, provider/schema mismatch and
any manifest that cannot be independently verified. It MUST never scan
arbitrary disks or trust a filename or unverified JSON. A fully verified
manifest may insert or repair its catalog projection; an absent, missing or
invalid artifact may be reported as unavailable but is never treated as a
verified reusable item. Reconciliation never starts, resumes or schedules a
workflow and never imports a partial as a completed artifact.

Catalog list, reconciliation and reveal do not reserve or mutate transient run
admission. The app owners expose a separate at-most-one reconciliation gate,
bounded catalog read transactions and identity-held safe-filesystem locator
resolution. Filesystem scanning and native reveal occur outside database
transactions and without holding a mutex guard across `.await`.

Preserved partials remain transient safe-filesystem data, not catalog records.
After an application restart, partial reuse is manual/new-run reuse only: the
user starts a new run, and the backend validates the complete artifact
fingerprint, source identity, mode, format/transcript policy and helper-lock
identity before copying compatible regular files into a new clean staging
attempt. No startup action, catalog reconciliation or Downloads-tab mount may
automatically continue a prior run.

## Migration and removal

YouTube planning and execution are expressed behind adapter contracts that can
be registered as Phase 2 `WorkflowPlanner` and `StepExecutor` implementations.
The stable scan-plan, occurrence, artifact-manifest and error types remain
provider-owned during migration.

After the durable runtime is available:

1. new YouTube submissions route to exactly one runtime;
2. an already-running transient run may finish or cancel in place;
3. transient state is never dual-written into durable tables or the verified
   artifact catalog;
4. the provider-domain catalog schema and safe-locator contract remain
   readable by the replacement planner/executor, or receive an explicit
   versioned migration with verified backup/rollback evidence;
5. packaged/native parity is proven; and
6. `workflow::transient` is removed after no callers remain. Removing the
   transient bridge does not remove the verified-artifact catalog or silently
   change its locators. An already-running transient run may finish or cancel
   in place, but it is never reconstructed as a durable workflow.

## Verification gates

Implementation is not accepted until all of the following pass:

- transition-table, stale-run, revision-order and terminal-race tests;
- double-start, pause/resume and cancel-before-next-item tests;
- lost-start-response, current-run recovery and dropped-event reconstruction;
- atomic scan/inspect/start interleavings, abandoned guards and global caption
  permits;
- child/grandchild termination on Cancel, Quit and updater restart;
- dirty-note plus active-process exit-barrier ordering and timeout tests;
- bounded stdout/stderr, timeout and malformed-helper-output tests;
- subset/restart reruns using stable per-item artifact fingerprints;
- leaf and nested reparse swaps against clean helper-visible staging;
- mixed legacy-provider plus YouTube release-build measurement;
- structural negative fixtures covering reverse, relative, braced, aliased and
  re-export dependency bypasses; and
- installed Windows UAT against the exact packaged helper identities.

## Consequences

This decision adds a small amount of temporary workflow code, but avoids a new
provider-local runtime and produces contracts that migrate directly into Phase
2. V1 remains intentionally ephemeral for execution: closing and reopening the
application does not restore a run, and no durable queue or workflow history is
promised. The narrowly permitted catalog records only verified published
artifacts and never restores or drives execution.
