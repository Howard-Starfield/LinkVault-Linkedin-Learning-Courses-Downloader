# Spec: Phase 0 source-layout migration

**Author:** LinkVault engineering

**Date:** 2026-07-26

**Status:** Approved

**Reviewer:** Howard Deng

**Related ADR:** [ADR-001](../architecture/adr-001-unified-workflow-modular-monolith.md)

**Related plan:** [Unified workflow migration plan](../architecture/unified-workflow-migration-plan.md)

## Context

The Tauri backend currently keeps LinkedIn modules at the crate root while
Coursera and Newspaper use their own root directories. Application lifecycle,
shared path storage and update behavior are also mixed with provider files.
The inconsistent layout obscures ownership and gives a future provider no
clean template.

This phase records and establishes the target module boundaries without
changing runtime behavior, database schemas, IPC names, credentials, download
outputs or user-facing behavior. Temporary compatibility exports are allowed
so physical ownership can improve before all internal imports are migrated.

## Functional requirements

- FR-1: The backend MUST place all provider-owned Rust source beneath
  `src/providers/<provider>/`.
- FR-2: LinkedIn Learning MUST have an explicit `providers/linkedin` module.
- FR-3: Tauri lifecycle, shared database initialization, application storage
  paths and updater behavior MUST live beneath `src/app/`.
- FR-4: The backend MUST contain a documented `src/workflow/` boundary for the
  future shared kernel.
- FR-5: Existing crate-root module paths MAY be preserved temporarily through
  compatibility exports.
- FR-6: Compatibility exports MUST delegate to the new owned modules and MUST
  contain no new behavior.
- FR-7: `lib.rs` MUST remain the composition root and MUST NOT acquire provider
  domain logic during the move.
- FR-8: The root `src/` directory MUST contain only `lib.rs`, `main.rs`, and
  architecture-owned module directories after the migration.
- FR-9: Architecture documentation MUST identify the decision, target layout,
  migration phases, dependency rules and rollback policy.
- FR-10: Existing Tauri command names and managed-state setup MUST remain
  unchanged.
- FR-11: A provider MUST NOT import another provider's internal Rust modules.

## Non-functional requirements

- NFR-1: All existing Rust tests MUST compile and pass after the move.
- NFR-2: `cargo fmt --check` and `cargo check --all-targets` MUST pass.
- NFR-3: The production frontend build MUST pass.
- NFR-4: The migration MUST make no SQLite schema or persisted-data changes.
- NFR-5: The migration MUST make no credential-storage or path-validation
  behavior changes.
- NFR-6: Git MUST detect provider files as moves where their contents are
  unchanged.
- NFR-7: A repeatable structural verification MUST fail if a future provider
  source file is added directly to the crate root.

## Acceptance criteria

### AC-1: Provider ownership (FR-1, FR-2, FR-8)

Given the migrated Rust source tree

When its top-level directories and files are enumerated

Then `providers/linkedin`, `providers/coursera`, and
`providers/newspaper` exist

And no provider implementation file exists directly under `src/`.

### AC-2: Application ownership (FR-3, FR-8)

Given the migrated Rust source tree

When application-level source is enumerated

Then updater, shared database, application storage-path and shared security
behavior are owned by `src/app/`

And `lib.rs` and `main.rs` are the only Rust files directly under `src/`.

### AC-3: Compatibility (FR-5, FR-6, FR-10, NFR-1)

Given existing internal imports and Tauri command registration

When the backend is compiled and its tests run

Then the existing module paths resolve through compatibility exports

And command names and managed states are unchanged.

### AC-4: No persistence behavior change (FR-10, NFR-4, NFR-5)

Given the source-layout migration diff

When schema declarations, token paths and storage constants are compared

Then their values and behavior are unchanged

And no migration or database column is added.

### AC-5: Structural regression guard (FR-1, FR-8, NFR-7)

Given the structural verification command

When an unexpected Rust source file is placed directly under `src/`

Then verification exits unsuccessfully and identifies the unexpected file.

### AC-6: Build verification (NFR-1, NFR-2, NFR-3)

Given the completed source-layout migration

When formatting, Rust tests, Rust checks and the frontend production build run

Then every command completes successfully.

### AC-7: Architecture record (FR-4, FR-9)

Given a future contributor unfamiliar with the migration

When they open `docs/architecture/README.md`

Then they can locate the accepted decision, authoritative roadmap, current
phase spec and module-boundary rules.

### AC-8: Provider isolation (FR-11, NFR-7)

Given the three provider source trees

When the structural verification scans Rust imports recursively

Then no provider imports another provider through either a crate-root
compatibility export or an internal `providers` path.

## Edge cases and error scenarios

- EC-1: A moved module uses `super::super` to reach a crate-root sibling. It
  MUST be changed to an explicit crate-owned compatibility path.
- EC-2: Git records an unchanged file as delete/add. Content equality MUST be
  verified; this does not by itself block the migration.
- EC-3: A structural check scans generated `target/` content. The check MUST
  inspect only direct source children.
- EC-4: A moved private command module becomes accidentally public. Existing
  effective visibility MUST be preserved.
- EC-5: A provider import crosses directly into another provider. The
  migration MUST NOT introduce the import; existing shared access remains
  through documented application or compatibility boundaries.
- EC-6: A build failure exposes a path-resolution difference. The move MUST be
  corrected or rolled back; behavioral code MUST NOT be changed to conceal it.

## API contracts

No Tauri IPC contract changes are allowed.

The Rust module compatibility contract for this phase is:

```rust
mod app;
mod providers;
pub mod workflow;

// Temporary compatibility exports preserve existing effective paths.
pub use app::{database as cache, security, storage};
pub use providers::{coursera, newspaper};
pub use providers::linkedin::{
    artifact_downloader, auth, browser_cookies, course,
    download_orchestrator, exercise_archive, live_clients, quality,
    quiz_hints, token_store,
};
```

Private composition aliases for `commands`, `linkedin` and updater wiring MAY
remain private.

## Data models

N/A - this phase performs no database schema, serialized model, command payload
or persisted-state changes.

## Out of scope

- OS-1: Implementing workflow tables or migrations. This belongs to Phase 2
  after the persistence foundation.
- OS-2: Changing queue, scheduler, retry, cancellation or recovery behavior.
- OS-3: Rewriting existing imports to final workflow ports.
- OS-4: Changing React polling or introducing the unified Activity UI.
- OS-5: Changing download formats, paths, media optimization or reader state.
- OS-6: Adding dependencies such as Tokio utilities, tracing or a CPU pool.
- OS-7: Removing compatibility exports before their consumers migrate.
- OS-8: Moving or rewriting the nested legacy Python downloader.
