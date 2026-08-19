# Frontend/Rust Ownership Boundary

**Status:** Active

**Related PRD:** [Responsive frontend and desktop-shell hardening](../specs/frontend-responsive-layout-hardening.md)

This document defines the authoritative ownership boundary between the LinkVault
frontend (React/TypeScript/CSS) and the Rust backend. All responsive layout,
sidebar resize, and shell hardening work must respect this boundary.

## Principles

1. **Presentation stays in the frontend.** DOM geometry, CSS layout, pointer and
   keyboard interaction, focus management, accessibility presentation, theme,
   and purely visual preferences are frontend concerns.

2. **Domain authority stays in Rust.** Filesystem access, database operations,
   credentials, network/provider behavior, download/retry/pause/cancellation,
   queue scheduling, resource governance, domain validation, and durable state
   transitions are Rust concerns.

3. **No domain logic in React for convenience.** The responsive implementation
   MUST NOT move any Rust-owned concern into React to make testing or layout
   easier. If a calculation is touched by responsive work and belongs in Rust,
   it must be moved to the appropriate Rust owner and exposed via typed IPC.

4. **Typed IPC adapters only.** New IPC for layout work, if any, belongs behind
   a typed adapter that marshals inputs and outputs without redefining domain
   decisions. Layout-only changes should require no new backend command.

## Frontend MAY own

- DOM and CSS geometry calculations
- Pointer and keyboard resize interaction
- Focus and accessibility presentation
- Theme and purely visual preferences
- Transient open/closed/selected UI state
- Formatting data that is already authoritative and typed for display
- Typed IPC adapters that marshal inputs and outputs without redefining domain decisions
- Container-responsive layout decisions based on available workspace width
- Sidebar width persistence (presentation-only UI state in localStorage)

## Rust MUST own

- Filesystem access and path safety
- Database reads/writes, migrations, and durable recovery
- Credentials and security decisions
- Network and provider behavior
- Download, retry, pause, cancellation, queue, scheduling, and resource-governor rules
- Domain validation and normalization whose result changes what work is accepted or executed
- Durable provider and application state transitions
- Calculations that determine scheduling, persistence, download behavior, or provider semantics

## Enforcement

The `verify:architecture` script enforces backend module boundaries. Frontend
ownership is enforced through code review against this document. Any new
frontend code that introduces domain logic must be flagged during review and
either moved to Rust or explicitly recorded as follow-up architecture debt.

## Anti-patterns

These patterns violate the ownership boundary and must not be introduced:

1. **Scheduling logic in React.** Date expansion, wait-range calculation, or
   queue ordering that determines when work executes belongs in Rust.

2. **Path construction in TypeScript.** File paths for downloads, snapshots, or
   archives must be constructed and validated in Rust.

3. **Credential handling in React.** Token storage, validation, or refresh
   logic must remain in Rust; the frontend only passes opaque values.

4. **Provider-specific business rules in views.** Course URL parsing, artifact
   planning, or edition catalog logic belongs in the provider's Rust module.

5. **Direct `invoke(...)` proliferation.** New IPC calls must use typed
   adapters. Layout-only changes should not introduce new commands.

## Migration of existing debt

Existing frontend domain calculations that predate this boundary may remain
temporarily. If this PRD's implementation touches such code, the default action
is to move the authoritative calculation to Rust. If migration is not feasible
within this work, it must be explicitly recorded as follow-up debt with a
tracking issue.

Browser-preview fixtures MAY emulate backend responses for UI testing, but the
emulation MUST be clearly marked as test/preview behavior and MUST NOT become
the production authority.
