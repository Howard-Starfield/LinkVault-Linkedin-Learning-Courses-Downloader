# Coursera Tab — Execution Plan (2026-06-12)

> Session-specific execution spec. The canonical architecture and isolation rules already live in:
> - `docs/coursera-tab-implementation.md` (14-phase plan)
> - `docs/learning/agent-harness-coursera/ISOLATION_RULES.md`
> - `docs/learning/agent-harness-coursera/REFERENCE_CONTRACT.md`
> - `docs/learning/agent-harness-coursera/PHASE_PLAN.md`
>
> This spec is the **execution ticket** for this session: what's already in, what to build next, and the hard "do not touch" list.

## Goal (from `/goal`)

> Reuse the existing LinkVault Tauri 2 app (LinkedIn Learning downloader) as the host. Add a Coursera downloader as a fully isolated sibling — its own Rust module, its own SQLite tables, its own Tauri commands, its own DPAPI token file, and its own React sidebar tab. No shared state, no shared structs, no edits to LinkedIn-side files. The Coursera port is a clean Rust rewrite of coursera-dl (Python), not a runtime dependency on it. Add, never modify.

## Architecture (locked)

- **Host:** existing `apps/desktop` Tauri 2 app. LinkedIn side stays untouched.
- **Coursera module:** `apps/desktop/src-tauri/src/coursera/` — fully isolated Rust module.
- **No shared structs:** LinkedIn `commands::LinkVaultState` is not extended; a new `coursera::commands::CourseraState` is added as a separate `tauri::State`.
- **SQLite tables:** `coursera_jobs`, `coursera_job_events`, `coursera_settings` — all `coursera_*` prefixed; the existing `settings`/`jobs`/`job_events` tables are read-only.
- **DPAPI file:** `linkvault.coursera.dpapi` — separate from `linkvault.li_at.dpapi`. A new `coursera::coursera_token_store` re-uses the Windows DPAPI primitive but does not edit `token_store.rs`.
- **Event channel:** `coursera://job-event` (8-variant `CourseEvent` tagged enum).
- **React sidebar:** new third tab between `LinkedIn Courses` and `History`. `App.tsx` gets one-line extension to the `activeView` union and a new `<SidebarItem>`.

## Hard isolation rules (do not violate)

| Forbidden to edit | Allowed in same file |
|---|---|
| `linkedin.rs`, `auth.rs`, `course.rs`, `artifact_downloader.rs`, `live_clients.rs`, `browser_cookies.rs`, `download_orchestrator.rs`, `quality.rs`, `quiz_hints.rs`, `exercise_archive.rs` | — |
| `cache.rs` (LinkedIn tables) | only an **additive** migration step for `coursera_*` tables |
| `storage.rs` (LinkedIn helpers) | only **additive** `coursera_*` helpers |
| `token_store.rs` | none — new file `coursera/coursera_token_store.rs` |
| `commands.rs` (LinkedIn surface) | none — new `coursera::commands` |
| `primitives.tsx`, `index.css` | none — reuse as-is; new tokens under `.coursera-*` namespaces only |
| `tauri.conf.json` (anything but `bundle.longDescription`) | Phase 14 only |
| `lib.rs` plugins block | only `invoke_handler!` and `setup` |
| `App.tsx` branches/header/sidebar layout | only union + new `<SidebarItem>` + new branch |

## Phase status (this session start)

| Phase | Status | Files |
|---|---|---|
| 0 — Module skeleton & Cargo wiring | ✅ done | `coursera/mod.rs`, `lib.rs` (private `mod coursera;`) |
| 1 — Core utilities + error type | ✅ done | `coursera/utils.rs` (with tests), `coursera/error.rs` (with tests) |
| 2 — Config types | ✅ done | `coursera/config.rs` (full types, parsers, validators, tests) |
| 3 — Constants, HTTP client, auth | ⏳ next | `define.rs`, `client.rs`, `auth.rs`, `coursera_token_store.rs` |
| 4 — Syllabus extraction | pending | `syllabus.rs` |
| 5 — Per-content-type extractors | pending | `extractors/{lecture,supplement,quiz,programming,notebook,resources}.rs`, `extractors/mod.rs` |
| 6 — Filter & filename formatting | pending | `filter.rs`, `format.rs` |
| 7 — Native downloader | pending | `downloader.rs` |
| 8 — Orchestrator | pending | `orchestrator.rs` |
| 9 — DB schema & job persistence | pending | `cache.rs` migration (additive) + `coursera/job.rs` |
| 10 — Tauri commands | pending | `coursera/commands.rs` + `lib.rs` registration |
| 11 — Frontend types & IPC | pending | `apps/desktop/src/lib/coursera/{types,ipc,events}.ts` |
| 12 — UI components | pending | `apps/desktop/src/components/coursera/*` + App.tsx union + SidebarItem |
| 13 — E2E + smoke | pending | `tests/coursera_e2e.rs` (wiremock, `#[ignore]`) |
| 14 — Polish & packaging | pending | icons, README, version bump, NSIS build |

## Execution strategy

- **Phase ordering is mandatory** per `PHASE_PLAN.md`. No skipping.
- **Each phase ends with `cargo test -p linkvault` green** for that phase's module.
- **No live network in unit tests.** Phase 3's `login`/`validate_cauth` and Phase 13's e2e are gated `#[ignore]`. Wiremock is the seam.
- **No edits to LinkedIn-side files.** `git diff --name-only` is the audit.
- **Add, never modify** is the one-line test for any change.

## Open questions to lock at Phase 3

1. **Async runtime:** use `async fn` + `reqwest` (already in `Cargo.toml`) inside the `coursera/` module. Tauri 2 commands are `async fn` natively. Locked.
2. **DPAPI:** new file `linkvault.coursera.dpapi` in the same data dir, written by a new `coursera::coursera_token_store` module that re-uses the Windows DPAPI primitive by calling the same `CryptProtectData` / `CryptUnprotectData` syscalls behind its own `pub fn`s. `token_store.rs` is not edited.
3. **Token store code path:** the new `coursera_token_store.rs` is a *sibling* of `token_store.rs`, not a refactor. Both call the same Win32 API but neither imports the other.

## What "done" looks like

- `cargo test -p linkvault` green (all 6 internal phase test files pass).
- `cargo check` from `apps/desktop/src-tauri` clean, no warnings on the new files.
- `git diff --name-only` shows changes only in:
  - `apps/desktop/src-tauri/src/coursera/**` (new files / filled stubs)
  - `apps/desktop/src-tauri/src/lib.rs` (only `mod coursera;` and `invoke_handler!` extensions)
  - `apps/desktop/src-tauri/src/cache.rs` (additive `coursera_*` migration only)
  - `apps/desktop/src-tauri/src/storage.rs` (additive `coursera_*` helpers only)
  - `apps/desktop/src/**` (new `coursera/` files; `App.tsx` union + one `<SidebarItem>`)
- No edits to any LinkedIn-side Rust file or `commands.rs` body.
- README, `bundle.longDescription`, and `package.json`/`Cargo.toml` versions bumped at Phase 14.
