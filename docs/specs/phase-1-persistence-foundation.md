# Spec: Phase 1 persistence foundation

**Author:** LinkVault engineering

**Date:** 2026-07-26

**Status:** Approved

**Reviewer:** Howard Deng

**Implementation status:** Checkpoints A through C are implemented and their
automated gates are green. Exact release-executable startup, migration,
provider-surface, shutdown and restart smoke is green. Installed-bundle and
authenticated provider-download UAT remain external completion gates.

**Checkpoint B/C addendum approved:** 2026-07-26 by the reviewer instruction
to continue through completion

**Related ADR:** [ADR-001](../architecture/adr-001-unified-workflow-modular-monolith.md)

**Related plan:** [Unified workflow migration plan](../architecture/unified-workflow-migration-plan.md)

## Context

LinkVault currently opens its application SQLite database from multiple
provider services. Some paths call the shared schema initializer for ordinary
runtime work, while Newspaper paths open raw connections with inconsistent
connection policy. Schema creation, provider migrations, runtime access and
restart recovery therefore share no explicit lifecycle boundary.

Phase 1 establishes that boundary before the generic workflow tables are
introduced. It is divided into three independently reviewable checkpoints:
Checkpoint A owns initialization, versioning, backup and connection policy;
Checkpoint B serializes application writes; Checkpoint C adds redacted
diagnostics and records the release-build contention baseline. Phase 1 is not
complete until all three checkpoint gates pass.

Existing provider schemas and user-facing behavior must remain compatible.
This phase must not introduce generic workflow tables or move provider
execution into the future supervisor.

## Functional requirements

### Checkpoint A: lifecycle, backup and connections

- FR-1: Tauri startup MUST invoke one explicit application-database
  initialization function before managing provider state.
- FR-2: Database schema level MUST be stored in `PRAGMA user_version`.
- FR-3: A new database MUST initialize directly at the current schema version
  without creating an empty backup.
- FR-4: A populated database below the current schema version MUST receive an
  online SQLite backup before any schema migration runs.
- FR-5: A migration backup MUST pass `PRAGMA integrity_check` before migration
  continues.
- FR-6: A database newer than the supported schema version MUST be rejected
  without schema changes.
- FR-7: The supported schema version MUST be written only after all existing
  provider schema initialization and migration functions succeed.
- FR-8: Ordinary runtime connections MUST NOT execute schema creation,
  migration or provider seed functions.
- FR-9: Application-database connections MUST use one connection policy:
  foreign keys enabled, a 5,000 ms busy timeout and FULL synchronous mode.
- FR-10: File-backed application databases MUST use WAL journal mode after
  initialization.
- FR-11: Production provider code MUST obtain application-database
  connections through the shared application database module.
- FR-12: Existing Tauri command names, request/response payloads and provider
  tables MUST remain unchanged.

### Checkpoint B: serialized writes

- FR-13: New shared application and workflow writes MUST pass through one
  owned writer boundary. Existing provider-local legacy write paths MUST be
  frozen in a reviewed structural baseline and migrated only with their
  provider cutover in Phases 3 through 5.
- FR-14: The writer boundary MUST execute each write request to completion
  before beginning the next write request.
- FR-15: Read-only queries MUST remain independent of the writer while WAL
  snapshot semantics permit the read.
- FR-16: Writer shutdown MUST reject new requests and resolve or fail every
  accepted request before the database owner terminates.
- FR-17: Writer tasks and provider services MUST NOT hold a database write
  transaction across network, image-processing or external-process work.
- FR-22: The structural persistence gate MUST reject any new direct legacy
  provider write site beyond the reviewed Phase 1 baseline.

### Checkpoint C: diagnostics and baseline

- FR-18: Database initialization, migration, backup, writer requests and
  contention failures MUST emit structured diagnostic spans.
- FR-19: Diagnostics MUST identify operation, provider, workflow/job identifier
  when available, elapsed time and safe error classification.
- FR-20: Diagnostics MUST NOT include credentials, cookies, authorization
  headers or raw provider payloads.
- FR-21: A release-build baseline MUST record database busy failures, writer
  queue depth, operation latency and UI stall evidence for Newspaper,
  LinkedIn, and mixed workloads.

## Non-functional requirements

- NFR-1: The focused persistence gate MUST pass on Windows using one command:
  `npm.cmd run verify:persistence`.
- NFR-2: The full release verification MUST continue to pass with all existing
  Rust tests.
- NFR-3: Every migration test MUST use an isolated temporary directory and
  MUST leave no files outside it.
- NFR-4: Backup verification MUST compare both SQLite integrity and preserved
  representative data.
- NFR-5: Reopening a current database through the runtime API MUST leave its
  database bytes unchanged when no application write is requested.
- NFR-6: The Checkpoint B contention test MUST complete 800 accepted writes
  from eight producers with zero lost rows and zero unhandled
  `SQLITE_BUSY` failures.
- NFR-7: The database layer MUST reject unsupported future schema versions
  before creating a backup or changing the database.
- NFR-8: Phase 1 MUST add no plaintext-secret columns, settings keys or
  diagnostic fields.
- NFR-9: Final performance thresholds MUST be based on Checkpoint C
  release-build measurements, not development-mode estimates.
- NFR-10: The writer diagnostic buffer MUST retain at most 512 events and
  MUST NOT grow without bound.
- NFR-11: Based on the recorded Windows release baseline, the synthetic gate
  MUST complete 800 writes within 5,000 ms with zero failures and its
  concurrent snapshot read within 250 ms on the verification machine.

## Acceptance criteria

### AC-1: New database initialization (FR-1, FR-2, FR-3, FR-7)

Given a database path that does not exist

When application initialization runs

Then all existing provider schemas are available

And `PRAGMA user_version` equals the supported schema version

And no pre-migration backup is created.

### AC-2: Legacy backup before migration (FR-4, FR-5, FR-7, NFR-4)

Given a populated version-zero database containing representative user data

When application initialization runs

Then a uniquely named backup is created beside the database

And the backup passes `PRAGMA integrity_check`

And the representative data is readable from the backup

And the source database reaches the supported schema version.

### AC-3: Current database reopening (FR-8, NFR-5)

Given a current initialized database with no pending application write

When it is opened and queried through the runtime connection API

Then its schema version and schema objects remain unchanged

And the database bytes remain unchanged after the connection closes.

### AC-4: Future schema rejection (FR-6, NFR-7)

Given a database whose `user_version` is greater than the supported version

When application initialization runs

Then initialization returns `UNSUPPORTED_SCHEMA_VERSION`

And no backup or schema change is created.

### AC-5: Connection policy (FR-9, FR-10)

Given an initialized file-backed application database

When an ordinary runtime connection opens

Then `foreign_keys` is `1`

And `busy_timeout` is `5000`

And `synchronous` is `2` for FULL

And the database journal mode is `wal`.

### AC-6: Provider connection ownership (FR-8, FR-11, FR-12, FR-22)

Given the production provider source trees

When the persistence structural gate scans code before test modules

Then no provider opens the LinkVault application database directly outside
the reviewed legacy-write baseline and the external browser-cookie adapter

And no provider calls the startup schema initializer.

And the existing Tauri command and provider table contracts remain unchanged.

### AC-7: Serialized contention (FR-13, FR-14, NFR-6, NFR-11)

Given eight concurrent producers submitting 100 uniquely keyed writes each

When the writer boundary processes every accepted request

Then all 800 rows exist exactly once

And no producer receives an unhandled `SQLITE_BUSY` failure.

### AC-8: Read/write separation (FR-15, FR-17, NFR-11)

Given an active long-running provider network or CPU step

When an independent application snapshot query runs while a writer request is
deliberately paused before commit

Then the query returns the previous committed snapshot without waiting for the
writer request to commit

And no database transaction spans the provider work.

### AC-9: Graceful writer shutdown (FR-16)

Given accepted writes followed by a shutdown request

When the writer shuts down

Then all accepted writes resolve successfully or with an explicit error

And subsequent writes are rejected deterministically.

### AC-10: Redacted diagnostics (FR-18, FR-19, FR-20, NFR-8)

Given database activity associated with saved provider credentials

When diagnostic events are captured

Then operation, safe identifiers, elapsed time and error class are present

And credential values and raw provider payloads are absent.

### AC-11: Release baseline (FR-21, NFR-9)

Given release-build Newspaper, LinkedIn and mixed workloads

When the approved Windows performance capture is executed

Then database contention, writer depth, operation latency and UI stall
evidence are recorded

And the resulting measurements define the next performance thresholds.

## Edge cases and error scenarios

- EC-1: The database path is unwritable. Initialization MUST fail before
  provider state is managed and MUST preserve the original database.
- EC-2: Backup creation fails. Migration MUST NOT start.
- EC-3: Backup integrity verification does not return exactly `ok`. Migration
  MUST NOT start and the failed backup path MUST be reported safely.
- EC-4: A migration fails after backup creation. `user_version` MUST remain
  below the supported version and the verified backup MUST remain available.
- EC-5: Initialization is retried after a partial idempotent provider
  migration. Existing migration guards MUST permit recovery without deleting
  provider data.
- EC-6: Two application instances attempt initialization. At most one may
  migrate; the other MUST receive an explicit lock/busy error rather than
  bypassing the migration.
- EC-7: A runtime reader encounters a busy writer. The connection MUST honor
  the configured busy timeout and return a classified error if the timeout
  expires.
- EC-8: The destination backup filename already exists. Initialization MUST
  select a new path and MUST NOT overwrite a prior backup.
- EC-9: Writer work panics or its owning thread terminates. Accepted callers
  MUST receive a closed/unavailable error rather than wait indefinitely.
- EC-10: Diagnostic formatting receives a secret-bearing error string. The
  structured layer MUST emit the safe classification without the raw value.
- EC-11: The diagnostic buffer exceeds 512 events. The oldest event MUST be
  evicted and accepted application work MUST continue.

## API contracts

HTTP method and path: N/A - this is an internal Tauri desktop persistence
boundary and adds no HTTP or Tauri IPC endpoint.
`POST /api/database/migrate` is an explicitly forbidden example: database
lifecycle work MUST remain inside desktop startup rather than become a remote
endpoint.

The Checkpoint A Rust contract is:

```rust
pub const CURRENT_SCHEMA_VERSION: i32;

pub struct DatabaseInitialization {
    pub from_version: i32,
    pub to_version: i32,
    pub backup_path: Option<PathBuf>,
}

pub fn initialize_database(
    path: &Path,
) -> Result<(Connection, DatabaseInitialization), DatabaseLifecycleError>;

pub fn open_runtime(path: &Path) -> rusqlite::Result<Connection>;
```

Conceptual result shape for documentation and test traceability:

```typescript
interface DatabaseInitializationResult {
  fromVersion: number;
  toVersion: number;
  backupPath: string | null;
}

interface DatabaseLifecycleFailure {
  code:
    | "DATABASE_IO"
    | "DATABASE_SQLITE"
    | "BACKUP_INTEGRITY_FAILED"
    | "UNSUPPORTED_SCHEMA_VERSION";
  safeMessage: string;
}
```

The approved Checkpoint B writer contract is:

```rust
#[derive(Clone)]
pub struct DatabaseWriter;

pub struct DatabaseWriteContext {
    pub operation: &'static str,
    pub provider: &'static str,
    pub workflow_id: Option<String>,
}

impl DatabaseWriter {
    pub fn start(
        path: PathBuf,
        diagnostics: DatabaseDiagnostics,
    ) -> Result<Self, DatabaseWriteError>;

    pub fn execute<T, F>(
        &self,
        context: DatabaseWriteContext,
        task: F,
    ) -> Result<T, DatabaseWriteError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseWriteError>
            + Send
            + 'static;

    pub fn shutdown(&self) -> Result<(), DatabaseWriteError>;
}
```

`DatabaseWriter::execute` accepts work only while the writer is running. One
owned worker thread holds the write connection and executes accepted closures
in receive order. `shutdown` drains accepted requests, rejects later requests
with `WRITER_CLOSED`, and joins the owner thread. A panic inside a request is
caught and returned as `WRITER_TASK_PANICKED`; it MUST NOT terminate the owner.

The approved Checkpoint C diagnostic contract is:

```typescript
interface DatabaseDiagnosticEvent {
  sequence: number;
  timestampMs: number;
  kind:
    | "initialization"
    | "migration"
    | "backup"
    | "writer_request"
    | "contention"
    | "shutdown";
  operation: string;
  provider: "app" | "linkedin" | "coursera" | "newspaper" | "workflow";
  workflowId: string | null;
  elapsedMs: number;
  queueDepth: number;
  outcome: "ok" | "error";
  errorClass: string | null;
}
```

The event contract intentionally has no free-form error-message or provider
payload field.

## Data models

No provider table is added or removed in Checkpoint A.

| Field or artifact | Type | Constraints |
|---|---|---|
| `PRAGMA user_version` | signed integer | `0..CURRENT_SCHEMA_VERSION`; values above current are rejected |
| Migration backup | SQLite database file | Unique path beside source; never overwrites; integrity must equal `ok` |
| `DatabaseInitialization.from_version` | signed integer | Exact version observed before migration |
| `DatabaseInitialization.to_version` | signed integer | Exact supported version after success |
| `DatabaseInitialization.backup_path` | optional path | Present only when a populated legacy database was backed up |
| `DatabaseWriter` request | in-memory command | Closure and typed result cross one owned worker thread; never persisted |
| `DatabaseDiagnosticEvent` | bounded in-memory record | Maximum 512; fixed safe fields only; no raw message or payload |
| Legacy write baseline | structural manifest | Exact reviewed production call sites; additions fail verification |

Checkpoint B introduces in-memory request types but no new persisted table.
Generic workflow tables remain Phase 2.

## Success definition and test gates

### Checkpoint A is successful when

- AC-1 through AC-6 and EC-1 through EC-8 have automated coverage.
- `npm.cmd run verify:persistence` passes.
- `cargo check --all-targets`, the full Rust suite, architecture verification,
  Newspaper performance contracts, production build and release verification
  pass.
- No Tauri IPC, provider schema or credential behavior changes.

### Checkpoint B is successful when

- AC-7 through AC-9 and EC-6, EC-7 and EC-9 pass under repeated contention.
- Shared application/workflow writes use the writer boundary and the reviewed
  legacy provider write baseline has no additions.
- Network, image and external-process work occurs outside transactions.

### Checkpoint C is successful when

- AC-10 and AC-11 plus EC-10 and EC-11 pass.
- A redacted diagnostic sample and release-build baseline report are recorded.
- The recorded baseline is
  [Windows persistence baseline, 2026-07-26](../performance/persistence-baseline-windows-2026-07-26.json).
- The exact release-executable smoke is
  [Windows native persistence smoke, 2026-07-26](../performance/persistence-native-smoke-windows-2026-07-26.json).
- Measured performance thresholds are added to this spec.

### Phase 1 is successful only when

All three checkpoints are complete and the installed native application passes
manual startup, migration, provider smoke, shutdown and restart UAT against a
backed-up copy of realistic user data.

## Out of scope

- OS-1: Generic workflow tables, step leases and workflow events. These belong
  to Phase 2.
- OS-2: Migrating Newspaper, LinkedIn or Coursera execution to the workflow
  supervisor.
- OS-3: Changing download formats, output paths, reader projections or
  provider authentication.
- OS-4: Removing legacy provider job tables.
- OS-5: Adding the unified Activity UI or changing frontend polling.
- OS-6: Dynamic plugins or external workflow services.
- OS-7: Declaring Phase 1 complete after Checkpoint A alone.
