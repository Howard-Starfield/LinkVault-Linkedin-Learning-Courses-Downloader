# Unified workflow migration plan

**Status:** Approved

**Approved:** 2026-07-26

**Architecture decision:** [ADR-001](adr-001-unified-workflow-modular-monolith.md)

This is the authoritative implementation roadmap for consolidating LinkVault's
LinkedIn Learning, Newspaper, Coursera, and future background workflows. Phase
specifications may add detail but must not contradict this plan without a new
ADR.

## Current-state evidence

- The Rust composition root registers three command surfaces and manages three
  independent provider states.
- LinkedIn has its own job, artifact, event, cancellation and recovery model.
- Newspaper has its own batches, jobs, page optimization tasks, schedules,
  revisions and recovery model.
- Coursera has its own job and event tables, status convention, cancellation
  state and UI-driven processing loop.
- The frontend starts background processing and refreshes broad bootstrap state
  during active work.
- Provider services open SQLite connections directly, so configuration,
  migrations and writer behavior are not controlled at one boundary.

Media virtualization and downscaling remain valid optimizations, but they do
not resolve these lifecycle and ownership problems.

## Target runtime

`WorkflowRuntime` is created once during Tauri setup. It owns:

- a supervisor that claims due work and recovers expired leases;
- a compile-time registry of workflow planners and step executors;
- cancellation tokens and tracked shutdown;
- resource-class concurrency controls;
- a durable workflow repository;
- an atomic artifact store;
- committed-state revision notifications; and
- structured workflow, step and attempt tracing.

SQLite work follows these rules:

1. Migrations run once during startup before the runtime accepts work.
2. A dedicated writer boundary serializes application writes.
3. Read queries use separately configured WAL-compatible connections.
4. Claim transactions are short and contain no network or file work.
5. State and its audit event commit in the same transaction.
6. Retry scheduling uses persisted timestamps rather than UI timers.

## Workflow contracts

Provider integrations implement two primary contracts:

```rust
trait WorkflowPlanner {
    fn workflow_type(&self) -> WorkflowType;
    fn validate(&self, request: WorkflowRequest) -> Result<ValidatedRequest, WorkflowError>;
    fn plan(&self, request: ValidatedRequest) -> Result<WorkflowPlan, WorkflowError>;
}

trait StepExecutor {
    fn step_type(&self) -> StepType;
    async fn execute(
        &self,
        context: StepContext,
        step: ClaimedStep,
    ) -> Result<StepOutcome, StepError>;
}
```

Additional ports cover the workflow repository, artifact store, provider
credentials, time, telemetry and revision notification. Concrete method
signatures are finalized in the kernel phase spec before implementation.

## Durable data model

### `workflow_runs`

| Field group | Required contents |
|---|---|
| Identity | ID, workflow type, provider, optional legacy origin and ID |
| Lifecycle | State, priority, cancel/pause timestamps, created/updated/completed timestamps |
| Request | Schema version, validated request JSON, output root |
| Failure | Stable error classification, safe message |
| Schedule | Optional schedule ID |

### `workflow_steps`

| Field group | Required contents |
|---|---|
| Identity | ID, run ID, stable step key and step type |
| Lifecycle | State, attempt, maximum attempts and next-attempt timestamp |
| Lease | Owner, expiry and bounded heartbeat timestamp |
| Resources | Network, disk, CPU, blocking I/O or external-process class |
| Progress | Current, total and unit |
| Recovery | Input, output and checkpoint JSON |
| Failure | Classification and safe message |

### Supporting tables

- `workflow_step_dependencies` stores normalized prerequisite edges.
- `workflow_artifacts` stores stable logical identifiers, temporary/final paths,
  state, byte size, checksum, media type and provider metadata.
- `workflow_events` stores ordered, append-only audit/outbox records.
- `workflow_schedules` stores cadence, timezone, request and next-run state.

Provider tables continue to own course metadata, syllabus data, newspaper
editions, pages, thumbnails, reading progress and other domain projections.

## State model

Run states:

```text
queued, running, paused, retry_wait, cancelling,
succeeded, succeeded_with_warnings, failed, cancelled
```

Step states:

```text
pending, ready, running, retry_wait,
succeeded, skipped, failed, cancelled
```

Rust enums, SQL constraints and one transition matrix must agree. Execution is
at-least-once; stable idempotency keys and atomic artifact publication provide
effectively-once visible results.

## Provider workflow templates

### Newspaper

```text
materialize schedule
  -> fetch edition manifest
  -> download page steps
  -> optimize page steps
  -> verify archive
  -> publish library projection
```

The existing optimization-task lease behavior is the initial reference, but
the provider-local scheduler and UI processing loop are retired after cutover.

### LinkedIn Learning

```text
validate credentials and URLs
  -> fetch course metadata
  -> plan artifacts
  -> download independent artifact steps
  -> generate supplementary artifacts
  -> verify and finalize
```

Existing course and artifact client traits should be adapted, not rewritten.

### Coursera

```text
validate credentials and class
  -> fetch syllabus
  -> plan lecture and resource steps
  -> download independent items
  -> verify and finalize
```

Processing becomes backend-owned and asynchronous. Provider status strings are
translated to common typed states at the compatibility boundary.

### Future provider

A built-in provider adds one provider module, planner, executor registrations,
contract tests and optional provider UI. It must not add another scheduler,
generic job/event table, cancellation runtime or React processing loop.

[ADR-003](adr-003-youtube-transient-workflow-bridge.md) permits a temporary
workflow-owned, non-durable bridge for YouTube V1 while Phase 2 is absent. The
exception does not authorize provider-local lifecycle ownership, durable
tables, retry scheduling or reuse by another provider. Phase 2 migration routes
new YouTube submissions to exactly one runtime and removes the bridge after
packaged/native parity and rollback gates pass.

## Frontend contract

Generic workflow commands will eventually be:

```text
submit_workflow
get_workflow_snapshot
get_workflow_detail
list_workflow_history
pause_workflow
resume_workflow
cancel_workflow
retry_workflow
```

Provider commands remain appropriate for preview, discovery, authentication
and reader-only behavior. React loads an initial snapshot, listens for a
committed revision and requests changes since its known revision. It does not
own a processing loop.

## Migration phases

### Phase 0: Record and establish boundaries

- Record the ADR, roadmap and phase specification.
- Consolidate Rust provider sources beneath `providers/`.
- Add `app/` and `workflow/` ownership boundaries.
- Preserve behavior through temporary compatibility exports.

Gate: formatting, structural contract, Rust tests and production frontend build
pass without database or Tauri command changes.

### Phase 1: Persistence foundation

- Add explicit versioned migrations and pre-migration online backup.
- Centralize database configuration.
- Introduce the dedicated writer and read boundary.
- Add structured tracing and redacted diagnostics.

Gate: existing providers behave unchanged and database contention, backup and
restoration tests pass.

### Phase 2: Workflow kernel

- Add domain types, transition matrix and persistence repositories.
- Add supervisor, leases, retry, cancellation and resource governance.
- Add a synthetic workflow used only for recovery and fault-injection tests.
- Adapt ADR-003 transient lifecycle and managed-process contracts behind the
  durable runtime without dual-writing transient state.

Gate: restart, duplicate claim, cancellation, disk failure and retry tests pass.

### Phase 3: Newspaper pilot

- Migrate page optimization first.
- Migrate page downloads, batches and schedules.
- Replace provider polling with workflow revisions.

Gate: a long mixed download/optimization workload stays responsive and
recovers without an orphaned running step.

### Phase 4: LinkedIn Learning

- Adapt existing discovery, course and artifact ports.
- Import or finish legacy nonterminal jobs safely.
- Migrate progress and history projections.

Gate: golden artifact fixtures, retry, pause, cancel and restart behavior match
or improve on the legacy engine.

### Phase 5: Coursera

- Adapt the existing downloader and syllabus logic.
- Remove synchronous/UI-owned batch processing.
- Normalize history and lifecycle state.

Gate: React never invokes a processing loop and restart recovery passes.

### Phase 6: Unified activity UI

- Add shared active-work and history projections.
- Remove full-bootstrap active-work polling.
- Keep provider-specific detail and reader surfaces.

Gate: incremental updates remain bounded with large histories.

### Phase 7: Retire compatibility paths

- Remove legacy schedulers, compatibility exports and obsolete tables only
  after a compatibility release.
- Validate backup restoration and installed/native UAT.

## Verification requirements

- Transition matrix unit and property tests.
- SQLite migration, contention and rollback integration tests.
- Fault injection at claim, download, write, rename and completion boundaries.
- Provider contract and golden-output tests.
- Release-build Windows performance traces for Newspaper, LinkedIn and mixed
  workloads.
- Native pause, resume, cancel, shutdown, restart, offline, authentication
  expiry and disk-full UAT.
- A structural check preventing new provider code at the crate root.

Concurrency values and responsiveness thresholds will be fixed only after the
Phase 1 release-build baseline. They must not be guessed from development mode.

## Rollback policy

- Back up before schema migration and prove the backup can be restored.
- Use `legacy_origin` plus `legacy_id` for idempotent imports.
- Route each newly submitted workflow to exactly one engine.
- Do not dual-write legacy and new workflow state.
- Retain legacy history as a read-only projection through one compatibility
  release.
- Keep provider cutovers independently reversible.
