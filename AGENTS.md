# Project guidance

## Architecture and stack

LinkVault is a Windows-first **Tauri 2** desktop app: one React/Vite frontend
and one Rust crate (`linkvault` / `linkvault_lib`) under `apps/desktop`. There
is no Cargo workspace and no Python runtime.

| Layer | Stack |
| --- | --- |
| Frontend | React 19, TypeScript 5.6 (`strict` + `noImplicitAny`), Vite, Tailwind 4 |
| Backend | Rust 2021, MSRV `1.77.2`, Tauri 2, tokio, rusqlite (bundled), reqwest (rustls, often blocking) |
| Data | SQLite via `app::database_writer` (single writer thread), local `LinkVaultData` next to the exe |
| Providers | `providers/{linkedin,coursera,newspaper}` — do not cross-import |

Architecture source of truth: `docs/architecture/README.md`. Ownership boundary:
`docs/architecture/frontend-rust-ownership-boundary.md`. Verify module layout
with `npm run verify:architecture`.

- Presentation, layout, and transient UI state stay in TypeScript.
- Paths, SQLite, credentials, downloads, scheduling, and domain validation stay in Rust.
- `lib.rs` is composition-only. New providers must not invent a fourth scheduler.
- `workflow/` is a placeholder for the shared kernel; do not duplicate queue/retry
  machinery inside a provider unless an ADR authorizes it.

## TypeScript safety

- Project-owned `.ts` and `.tsx` files must not use explicit `any`, including `as any`, `Record<string, any>`, `Promise<any>`, or equivalent generic forms.
- Use `unknown` at untrusted boundaries, then narrow it with a type guard, a schema check, or a discriminated union before use.
- Do not use `@ts-ignore` or `@ts-nocheck` to bypass the compiler. Use `@ts-expect-error` only for a narrowly scoped, documented incompatibility that is covered by a focused check.
- Keep `strict` and `noImplicitAny` enabled in the desktop TypeScript configuration.
- Before handing off TypeScript changes, run `npm run verify:no-any` and `npm run build`. A failure in either check blocks completion.

## Rust safety

These rules exist to stop the recurring Rust mistakes in this crate: panics in
production paths, blocking I/O on the async runtime, SQLite writes off the
writer thread, Windows path/junction bugs, and silent Tauri/TypeScript contract
drift. Follow them whenever editing `apps/desktop/src-tauri`.

### Ownership and errors

- Prefer `Result<T, E>` and typed `thiserror` errors inside services. Map to
  `Result<T, String>` only at the Tauri command edge, keeping useful context.
- Do not use `unwrap()`, `expect()`, or `panic!()` on recoverable production
  paths. Tests may use them; do not copy that habit into `#[tauri::command]` or
  provider services.
- Do not "fix" the borrow checker with reflexive `.clone()`, `Arc<Mutex<_>>`,
  leaked `'static` refs, or `unsafe`. Clone only when the value is deliberately
  duplicated and the cost is understood.
- Do not add `unsafe` unless safe Rust cannot reasonably solve the problem.
  Keep the block minimal, document every safety invariant, and test the boundary.

### Async, blocking I/O, and locks

- Never hold a synchronous `Mutex`/`RwLock` guard across `.await`.
- Providers often use `reqwest::blocking` and filesystem I/O. Run that work on
  `tauri::async_runtime::spawn_blocking` (or a dedicated thread such as
  `DatabaseWriter`). Do not call blocking HTTP, SQLite, or heavy FS on the Tokio
  / Tauri async executor — it stalls the UI.
- Do not introduce `Arc`, `Mutex`, channels, or background tasks unless there
  is a concrete ownership or concurrency need. Document who owns mutable state.

### Persistence and Windows paths

- All SQLite writes go through `DatabaseWriter`. Do not open a second writer
  connection or mutate the DB from setup/async command bodies.
- Schema changes need a migration, compatibility/rollback note, and tests.
  Use transactions for multi-step writes. Run `npm run verify:persistence` when
  touching the writer, migrations, or clipping durability.
- Construct and validate filesystem paths in Rust, not TypeScript. Canonicalize
  where required; reject symlink/junction/reparse points for data roots and
  clipping assets. Keep archives inside `app/security.rs` path-safety checks.
- App data lives next to the exe under `LinkVaultData` (override:
  `LINKVAULT_DATA_DIR`). Do not write secrets to logs; tokens stay
  provider-owned and DPAPI-protected.

### Tauri IPC

- Commands stay thin: validate input, then delegate to an existing
  manager/service. Do not add a duplicate `State<T>` owner for the same resource.
- Treat frontend input, filenames, URLs, zip members, and network bodies as
  untrusted. No path traversal, SQL concatenation, or capability widening
  "to make it work".
- Changing a command name, argument, or payload requires updating the typed
  TypeScript adapter in the same change. Do not weaken TS to `any` to paper
  over a mismatch. Prefer typed adapters over raw `invoke(...)` in `App.tsx`.

### Dependencies and architecture

- Search the crate before adding helpers, traits, managers, or crates. New
  dependencies need a reason the stdlib/current `Cargo.toml` is insufficient,
  plus size/native/security impact. Verify APIs against this repo's crate
  versions, not memory of another version.
- Do not add a new provider scheduler, job table, or processing loop. Put
  shared workflow behavior in `workflow/` only when an ADR says so.

### Validation after Rust changes

From the repo root, using this crate's manifest (not a workspace):

- `npm run cargo:clippy`
- `npm run cargo:test`
- plus `npm run verify:architecture` / `npm run verify:persistence` when those
  boundaries moved

`cargo fmt --check` is not yet a green baseline (`docs/work-orders/rustfmt-baseline.md`).
Format only files you touch; do not reformat the crate. Clippy currently reports
pre-existing warnings — do not add new ones, and do not fail the tree on
`-D warnings` until that baseline is cleaned up.

A bug fix should include a regression test that would have failed before the fix.

## UI changes

- Preserve unrelated dirty work and generated files.
- For newspaper reader or responsive layout changes, run `npm run verify:ui` and the relevant browser or visual verification. Confirm behavior at narrow, compact, and wide container widths; a passing TypeScript build does not prove rendered geometry.
