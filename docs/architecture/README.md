# LinkVault Architecture

This directory is the source of truth for LinkVault's desktop architecture.
Implementation plans and code reviews must begin here instead of inferring the
design from whichever provider was implemented most recently.

## Current and proposed decisions

- [ADR-001: Unified workflow modular monolith](adr-001-unified-workflow-modular-monolith.md)
- [ADR-002: Newspaper clippings as provider-owned managed assets](adr-002-newspaper-clippings-managed-assets.md)
- [Newspaper Clippings V1 master PRD](../specs/newspaper-clippings-v1/README.md)
- [Unified workflow migration plan](unified-workflow-migration-plan.md)
- [Phase 0 source-layout migration spec](../specs/phase-0-source-layout-migration.md)
- [Phase 1 persistence foundation spec](../specs/phase-1-persistence-foundation.md)
- [Phase 1 Windows release baseline](../performance/persistence-baseline-windows-2026-07-26.json)
- [Phase 1 Windows native smoke](../performance/persistence-native-smoke-windows-2026-07-26.json)

The status inside each ADR or specification is authoritative. A linked document
may still be Proposed and therefore does not authorize implementation.

## Target backend layout

```text
apps/desktop/src-tauri/src/
  app/                    Tauri lifecycle, shared database and application services
  workflow/               Shared durable workflow kernel
    domain/               State and transition rules
    application/          Planning, supervision, retry and cancellation
    ports/                Repository and external-service contracts
    infrastructure/       SQLite, runtime, filesystem and telemetry adapters
  providers/
    linkedin/             LinkedIn-specific discovery and artifact behavior
    coursera/             Coursera-specific discovery and artifact behavior
    newspaper/            Newspaper-specific catalog, download and reader behavior
  lib.rs                  Composition root only
  main.rs                 Executable entry point only
```

The source tree will reach this layout incrementally. A directory's presence
does not imply that its future behavior has already been implemented.

## Boundary rules

1. A provider MAY depend on public workflow ports and shared application
   services.
2. The workflow kernel MUST NOT depend on a provider.
3. One provider MUST NOT import another provider's internal modules.
4. `lib.rs` MUST remain the composition root rather than acquire domain logic.
5. New providers MUST NOT create a new scheduler, generic job table,
   cancellation runtime, or frontend processing loop.
6. Provider-specific domain data MUST remain provider-owned.
7. Credentials MUST remain provider-owned opaque values and MUST NOT enter
   workflow payloads, events, or logs.

Temporary crate-root compatibility exports are permitted only during the
strangler migration. Each compatibility export must point into one of the
owned modules above and must be removed after its consumers migrate.
